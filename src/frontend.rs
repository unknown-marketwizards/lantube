use axum::{
    body::Body,
    extract::Path,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};

include!(concat!(env!("OUT_DIR"), "/frontend_assets.rs"));

pub async fn index() -> Response {
    respond(
        "index.html",
        embedded_asset("index.html").expect("index.html must be embedded"),
    )
}

pub async fn asset(Path(path): Path<String>) -> Result<Response, StatusCode> {
    let bytes = embedded_asset(&path).ok_or(StatusCode::NOT_FOUND)?;
    Ok(respond(&path, bytes))
}

fn respond(path: &str, bytes: &'static [u8]) -> Response {
    let content_type = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type.as_ref()).expect("MIME type must be a valid header"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}
