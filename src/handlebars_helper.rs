use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use handlebars::{
    Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderErrorReason,
};
use std::process::Command;
use std::process::Stdio;

/// Check if a value exists in an array using strict equality comparison
///
/// This helper enables conditional Handlebars templates by checking array membership.
/// Performs type-strict comparisons using serde_json::Value equality semantics.
///
/// * `value` - The value to search for (any JSON type)
/// * `array` - The array to search through (must be a JSON array)
///
/// # Errors
/// Returns a render error if:
/// - Missing required parameters (needs exactly 2 parameters)
/// - Second parameter is not an array type
///
/// # Example
/// ```handlebars
/// {{#contains "nginx" services}}
///   {{> nginx-config}}
/// {{/contains}}
/// ```
pub(crate) fn contains_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    // Validate parameter count first to fail fast on invalid usage
    let params = h.params();
    if params.len() != 2 {
        return Err(RenderErrorReason::Other(format!(
            "contains helper requires exactly 2 parameters (got {})",
            params.len()
        ))
        .into());
    }

    // Extract search value from first parameter using Value comparison
    let value = params[0].value();

    // Validate array parameter type before iteration
    let array_val = params[1].value();
    let array = array_val.as_array().ok_or_else(|| {
        RenderErrorReason::Other(format!(
            "contains helper second parameter must be an array (got {})",
            array_val
        ))
    })?;

    // Perform type-strict comparison with array elements
    let contains = array.iter().any(|item| item == value);

    // Output "true" string when found to work with Handlebars conditional blocks
    if contains {
        out.write("true")?;
    }

    Ok(())
}

pub(crate) fn is_executable_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let mut params = h.params().iter();
    let executable = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("is_executable", 0))?
        .render();
    if params.next().is_some() {
        return Err(RenderErrorReason::Other(
            "is_executable: More than one parameter given".to_owned(),
        )
        .into());
    }

    let status = is_executable(&executable)
        .map_err(|e| RenderErrorReason::Other(format!("Failed to run is_executable: {e}")))?;
    if status {
        out.write("true")?;
    }
    // writing anything other than an empty string is considered truthy

    Ok(())
}

fn is_executable(name: &str) -> Result<bool> {
    Command::new("which")
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .wrap_err_with(|| eyre!("Failed to run `which {}`", name))
}

pub(crate) fn find_executable_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let mut params = h.params().iter();
    let executable = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("is_executable", 0))?
        .render();
    if params.next().is_some() {
        return Err(RenderErrorReason::Other(
            "is_executable: More than one parameter given".to_owned(),
        )
        .into());
    }

    let output = find_executable(&executable)
        .map_err(|e| RenderErrorReason::Other(format!("Failed to run is_executable: {e}")))?;
    if !output.is_empty() {
        out.write(&String::from_utf8_lossy(&output).to_string())?;
    }
    // writing anything other than an empty string is considered truthy

    Ok(())
}
fn find_executable(name: &str) -> Result<Vec<u8>> {
    Command::new("which")
        .arg(name)
        .output()
        .map(|s| s.stdout)
        .wrap_err_with(|| eyre!("Failed to run `which {}`", name))
}


pub(crate) fn command_success_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let mut params = h.params().iter();
    let command = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex(
            "command_success",
            0,
        ))?
        .render();
    if params.next().is_some() {
        return Err(RenderErrorReason::Other(
            "command_success: More than one parameter given".to_owned(),
        )
        .into());
    }

    let status = os_shell()
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();
    if status {
        out.write("true")?;
    }
    // writing anything other than an empty string is considered truthy

    Ok(())
}

pub(crate) fn command_output_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let mut params = h.params().iter();
    let command = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex(
            "command_success",
            0,
        ))?
        .render();
    if params.next().is_some() {
        return Err(RenderErrorReason::Other(
            "command_success: More than one parameter given".to_owned(),
        )
        .into());
    }

    let output = os_shell()
        .arg(&command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // .stderr(Stdio::piped()) - probably not wanted
        .output()?;
    out.write(&String::from_utf8_lossy(&output.stdout))?;
    // writing anything other than an empty string is considered truthy

    Ok(())
}

fn os_shell() -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c");
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use color_eyre::Result;
    use serde_json::json;

    fn setup_handlebars() -> Handlebars<'static> {
        let mut hb = Handlebars::new();
        hb.register_helper("contains", Box::new(contains_helper));
        hb
    }

    #[test]
    fn test_contains_helper() -> Result<()> {
        let hb = setup_handlebars();

        // Test basic string array functionality
        let template = "{{#contains 'apple' fruits}}{{/contains}}";
        let ctx = json!({"fruits": ["apple", "banana", "orange"]});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "true",
            "should find element in string array"
        );

        // Test element not found case
        let ctx = json!({"fruits": ["grape", "banana"]});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "",
            "should not find missing element"
        );

        // Test numeric types and type matching
        let template = "{{#contains 42 numbers}}{{/contains}}";
        let ctx = json!({"numbers": [42, 3.5, 100]});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "true",
            "should match numeric types exactly"
        );

        // Test case sensitivity
        let ctx = json!({"fruits": ["apple", "banana"]});
        assert_eq!(
            hb.render_template("{{#contains 'Apple' fruits}}{{/contains}}", &ctx)?,
            "",
            "should be case-sensitive for string values"
        );

        // Test special values (null/empty/boolean)
        let ctx = json!({"values": [null, "test"]});
        assert_eq!(
            hb.render_template("{{#contains null values}}{{/contains}}", &ctx)?,
            "true",
            "should handle null values"
        );

        let ctx = json!({"values": []});
        assert_eq!(
            hb.render_template("{{#contains 1 values}}{{/contains}}", &ctx)?,
            "",
            "should handle empty array"
        );

        let ctx = json!({"flags": [true, false]});
        assert_eq!(
            hb.render_template("{{#contains true flags}}{{/contains}}", &ctx)?,
            "true",
            "should match boolean values"
        );

        // Test error conditions
        let template_missing_params = "{{#contains 'apple'}{{/contains}}";
        let result = hb.render_template(template_missing_params, &json!({}));
        assert!(
            result.is_err(),
            "should error when missing required parameters"
        );

        let template_extra_params = "{{#contains 1 2 3}}{{/contains}}";
        let result = hb.render_template(template_extra_params, &json!({}));
        assert!(result.is_err(), "should reject too many parameters");

        // Test non-array parameter error
        let result = hb.render_template(
            "{{#contains 'foo' not_an_array}}{{/contains}}",
            &json!({"not_an_array": "string"}),
        );
        assert!(
            result.is_err(),
            "should error when second parameter is not an array"
        );

        Ok(())
    }
}
