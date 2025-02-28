use handlebars::{
    Context, Handlebars, Helper, HelperResult, JsonValue, Output, RenderContext, RenderErrorReason,
};

use serde_json::json;

pub(crate) fn contains_helper(
    h: &Helper<'_>,
    _: &Handlebars<'_>,
    _: &Context,
    _: &mut RenderContext<'_, '_>,
    out: &mut dyn Output,
) -> HelperResult {
    let mut params = h.params().iter();
    // Get the value to check
    let value = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("value", 0))?
        .value();

    // Get the array to check against
    let array_default = json!([]);
    let array = params
        .next()
        .ok_or(RenderErrorReason::ParamNotFoundForIndex("array", 1))?
        .value().as_array().unwrap_or(array_default.as_array().unwrap());

    if params.next().is_some() {
        return Err(
            RenderErrorReason::Other("contains: More than 2 parameters given".to_string()).into(),
        );
    }

    // Check if the value is in the array
    let contains = array.iter().any(|item| item == value);

    // Render the appropriate block
    if contains {
        out.write("true")?;
    }

    Ok(())
}
