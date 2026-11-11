//! Frozen API vocabulary and codecs retained for client compatibility.
// Published API vocabulary/exports remain intact even for unadvertised Core actions.
#[allow(dead_code, unused_imports)]
pub mod schema;

#[cfg(test)]
pub(crate) fn request_changes_ui(request: &schema::Request) -> bool {
    request.method.operation().changes_ui
}

pub(crate) fn api_method_name(method: &schema::Method) -> &'static str {
    method.operation().name
}
