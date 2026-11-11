use crate::error::AppError;
use anyhow::{Result, ensure};
use axum::{
    Extension,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use ctutils::CtEq;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};

const WRITE_HEADER: &str = "x-lantube-write";
const CAN_WRITE_HEADER: &str = "x-lantube-can-write";

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    #[default]
    Viewer,
    Editor,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserConfig {
    password: String,
    #[serde(default)]
    role: Role,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    users: HashMap<String, UserConfig>,
}

struct Account {
    digest: [u8; 32],
    role: Role,
}

#[derive(Clone, Default)]
pub struct Auth(Option<Arc<HashMap<String, Account>>>);

impl AuthConfig {
    pub fn build(self) -> Result<Auth> {
        ensure!(!self.users.is_empty(), "Configure at least one auth user");
        let mut accounts = HashMap::with_capacity(self.users.len());
        for (name, user) in self.users {
            ensure!(
                !name.trim().is_empty()
                    && !name.contains(':')
                    && !name.chars().any(char::is_control),
                "Invalid auth username"
            );
            ensure!(
                !user.password.trim().is_empty() && !user.password.chars().any(char::is_control),
                "Invalid password for auth user {name:?}"
            );
            accounts.insert(
                name,
                Account {
                    digest: Sha256::digest(user.password.as_bytes()).into(),
                    role: user.role,
                },
            );
        }
        Ok(Auth(Some(Arc::new(accounts))))
    }
}

#[derive(Clone, Copy)]
pub struct Access(Option<Role>);

impl Access {
    fn can_write(self) -> bool {
        !matches!(self.0, Some(Role::Viewer))
    }
}

fn role_for(accounts: &HashMap<String, Account>, headers: &HeaderMap) -> Option<Role> {
    let (scheme, token) = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }
    let decoded = STANDARD.decode(token.trim_start_matches(' ')).ok()?;
    let (name, password) = std::str::from_utf8(&decoded).ok()?.split_once(':')?;
    let account = accounts.get(name)?;
    let digest: [u8; 32] = Sha256::digest(password.as_bytes()).into();
    bool::from(account.digest.ct_eq(&digest)).then_some(account.role)
}

pub async fn authenticate(State(auth): State<Auth>, mut request: Request, next: Next) -> Response {
    let role = if let Some(accounts) = &auth.0 {
        let Some(role) = role_for(accounts, request.headers()) else {
            return (
                StatusCode::UNAUTHORIZED,
                [
                    (
                        header::WWW_AUTHENTICATE,
                        "Basic realm=\"LanTube\", charset=\"UTF-8\"",
                    ),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                "Authentication required",
            )
                .into_response();
        };
        Some(role)
    } else {
        None
    };

    request.extensions_mut().insert(Access(role));
    let mut response = next.run(request).await;
    if role.is_some() {
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    response
}

pub async fn require_write(
    Extension(access): Extension<Access>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    if !access.can_write() {
        return Err(AppError::forbidden("Write access required"));
    }
    // 跨站表单无法附带这个请求头，跨站脚本须先通过 CORS 预检。
    if access.0.is_some()
        && !request
            .headers()
            .get(WRITE_HEADER)
            .is_some_and(|value| value == "1")
    {
        return Err(AppError::forbidden("Missing LanTube write header"));
    }
    Ok(next.run(request).await)
}

pub async fn expose_access(
    Extension(access): Extension<Access>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        CAN_WRITE_HEADER,
        HeaderValue::from_static(if access.can_write() { "true" } else { "false" }),
    );
    response
}

pub async fn sandbox_file(
    Extension(access): Extension<Access>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    if access.0.is_some() {
        // 共享目录里的 HTML 等文件不能以 LanTube 的身份执行脚本。
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("sandbox"),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_incomplete_or_invalid_account_configuration() {
        for source in [
            "",
            "[users.reader]",
            "[users.reader]\npassword = 'pass'\nrole = 'owner'",
        ] {
            assert!(toml::from_str::<AuthConfig>(source).is_err());
        }
        for source in [
            "[users]",
            "[users.reader]\npassword = ''",
            "[users.'bad:name']\npassword = 'pass'",
        ] {
            let config: AuthConfig = toml::from_str(source).unwrap();
            assert!(config.build().is_err());
        }
    }
}
