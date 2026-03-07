use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use handlebars::{
    Context, Handlebars, Helper, HelperResult, Output, RenderContext, RenderErrorReason,
};
use std::process::Command;
use std::process::Stdio;

/// Check if a value exists in an array or matches any of the provided values.
///
/// This helper supports two usage patterns:
/// 1. With array variable: `{{contains value array}}` - checks if value is in the array
/// 2. Variadic: `{{contains value item1 item2 item3}}` - checks if value matches any item
///
/// Performs type-strict comparisons using serde_json::Value equality semantics.
///
/// # Example
/// ```handlebars
/// {{#contains "nginx" services}}...{{/contains}}
/// {{#if (contains DOD_DISTRIBUTION_NAME 'arch' 'banana' 'orange')}}...{{/if}}
/// ```
pub(crate) fn contains_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let params = h.params();
    if params.is_empty() {
        return Err(RenderErrorReason::Other(
            "contains helper requires at least 1 parameter".to_string(),
        )
        .into());
    }

    // Extract search value from first parameter
    let value = params[0].value();

    // Check remaining parameters for a match
    let contains = params.iter().skip(1).any(|param| {
        let param_val = param.value();
        // If the parameter is an array, check within it
        if let Some(arr) = param_val.as_array() {
            arr.iter().any(|item| item == value)
        } else {
            // Otherwise, compare directly
            param_val == value
        }
    });

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
        out.write(String::from_utf8_lossy(&output).as_ref())?;
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

        // Test basic string array functionality (with array variable)
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

        // Test variadic usage - multiple literal values
        let template = "{{#contains 'arch' 'arch' 'banana' 'orange'}}{{/contains}}";
        let ctx = json!({});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "true",
            "should find element in variadic arguments"
        );

        // Test variadic - element not found
        let template = "{{#contains 'fedora' 'arch' 'banana' 'orange'}}{{/contains}}";
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "",
            "should not find missing element in variadic arguments"
        );

        // Test with variable and variadic literals (typical use case from config)
        let template =
            "{{#if (contains DOD_DISTRIBUTION_NAME 'arch' 'banana')}}true{{else}}false{{/if}}";
        let ctx = json!({"DOD_DISTRIBUTION_NAME": "arch"});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "true",
            "should work with variable and variadic literals in condition"
        );

        // Test with variable not matching
        let ctx = json!({"DOD_DISTRIBUTION_NAME": "fedora"});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "false",
            "should return false when variable doesn't match any literal"
        );

        // Test mixed: variable and array variable
        let template = "{{#contains DOD_DISTRIBUTION_NAME distros}}{{/contains}}";
        let ctx = json!({"DOD_DISTRIBUTION_NAME": "arch", "distros": ["arch", "fedora", "ubuntu"]});
        assert_eq!(
            hb.render_template(template, &ctx)?,
            "true",
            "should work with variable and array variable"
        );

        Ok(())
    }
}
