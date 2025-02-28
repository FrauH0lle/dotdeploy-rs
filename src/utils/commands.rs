use color_eyre::Result;
use color_eyre::eyre::{OptionExt, WrapErr};
use std::ffi::{OsStr, OsString};
use std::process::{Output, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Executes a command and returns its output.
///
/// # Arguments
///
/// * `cmd` - The command to execute.
/// * `args` - Arguments for the command.
///
/// # Errors
/// Returns an error if the command fails to execute.
pub(crate) async fn exec_output<S, I>(cmd: S, args: I) -> Result<Output>
where
    S: AsRef<OsStr>,
    I: IntoIterator<Item = S>,
{
    let cmd_os = cmd.as_ref();
    // Collect args into OsString once (required for Command::args)
    let args_os: Vec<OsString> = args
        .into_iter()
        .map(|a| a.as_ref().to_os_string())
        .collect();

    let mut run = Command::new(cmd_os)
        .args(&args_os)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .wrap_err_with(|| {
            // Convert to strings ONLY if an error occurs
            let cmd_str = cmd_os.to_string_lossy();
            let args_str: Vec<String> = args_os
                .iter()
                .map(|a| a.to_string_lossy().to_string())
                .collect();
            format!("Failed to spawn {} {}", cmd_str, args_str.join(" "))
        })?;

    let stdout = run.stdout.take().ok_or_eyre("Stdout handle present")?;
    let stderr = run.stderr.take().ok_or_eyre("Stderr handle present")?;

    // Spawn tasks to read and forward output streams
    let stdout_handle = tokio::task::spawn(capture_stream(stdout, tokio::io::stdout()));
    let stderr_handle = tokio::task::spawn(capture_stream(stderr, tokio::io::stderr()));

    let status = run.wait().await.wrap_err_with(|| {
        // Convert to strings ONLY if an error occurs
        let cmd_str = cmd_os.to_string_lossy();
        let args_str: Vec<String> = args_os
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        format!("Failed to execute {} {}", cmd_str, args_str.join(" "))
    })?;
    let (stdout_result, stderr_result) = tokio::join!(stdout_handle, stderr_handle);

    Ok(Output {
        status,
        stdout: stdout_result.wrap_err("Stdout task panicked")?,
        stderr: stderr_result.wrap_err("Stderr task panicked")?,
    })
}

/// Captures data from an input stream while simultaneously writing it to an output stream.
///
/// This async function reads data from the provided input stream in chunks, writes it to the
/// specified output stream immediately, and collects all read data into a buffer.
///
/// # Arguments
/// * `stream` - The input stream to read from (must implement AsyncRead + Unpin)
/// * `writer` - The output stream to write to (must implement AsyncWrite + Unpin)
///
/// # Returns
/// A `Vec<u8>` containing all captured bytes from the input stream.
pub(crate) async fn capture_stream<R, W>(stream: R, mut writer: W) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // Initialize buffer to store captured data
    let mut buffer = Vec::new();

    // Create buffered reader for efficient async reading
    let mut reader = tokio::io::BufReader::new(stream);

    // Buffer for reading chunks of data (1KB chunks)
    let mut chunk = vec![0; 1024];

    loop {
        // Read chunk from input stream
        match reader.read(&mut chunk).await {
            Ok(0) => {
                // End of stream reached
                break;
            }
            Ok(n) => {
                // Successfully read n bytes
                let data = &chunk[..n];

                // Write data to output stream immediately
                writer.write_all(data).await.ok();

                // Store captured data in buffer
                buffer.extend_from_slice(data);
            }
            Err(e) => {
                // Handle read errors
                eprintln!("Error reading stream: {}", e);
                break;
            }
        }
    }
    buffer
}
