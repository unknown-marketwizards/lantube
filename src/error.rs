use axum::extract::multipart::MultipartError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        let error: anyhow::Error = error.into();

        if let Some(error) = error.downcast_ref::<MultipartError>() {
            return Self {
                status: error.status(),
                message: error.body_text(),
            };
        }

        let status = match error
            .downcast_ref::<std::io::Error>()
            .map(|error| error.kind())
        {
            Some(std::io::ErrorKind::NotFound) => StatusCode::NOT_FOUND,
            Some(std::io::ErrorKind::AlreadyExists) => StatusCode::CONFLICT,
            Some(std::io::ErrorKind::PermissionDenied) => StatusCode::FORBIDDEN,
            Some(std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotADirectory) => {
                StatusCode::BAD_REQUEST
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::{FromRequest, Multipart};
    use axum::http::{Request, header};

    #[tokio::test]
    async fn preserves_payload_too_large_status() {
        let content = "x".repeat(2 * 1024 * 1024);
        let body = format!(
            "--boundary\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n{content}\r\n\
             --boundary--\r\n"
        );
        let request = Request::post("/")
            .header(
                header::CONTENT_TYPE,
                "multipart/form-data; boundary=boundary",
            )
            .body(Body::from(body))
            .unwrap();
        let mut multipart = Multipart::from_request(request, &()).await.unwrap();
        let error = AppError::from(multipart.next_field().await.unwrap_err());

        assert_eq!(
            error.into_response().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
