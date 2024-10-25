use std::fmt::Display;

use axum::{http::StatusCode, response::IntoResponse};

pub mod books;
pub mod config;
pub mod dict;
pub mod doc;
pub mod dt;
pub mod morph;

#[derive(Debug)]
pub enum Error {
    AppError(anyhow::Error),
    Status(StatusCode, String),
}

#[macro_export]
macro_rules! time {
    ($expr:expr) => {
        {
            let now = std::time::Instant::now();
            let x = $expr;
            dbg!(now.elapsed());
            x
        }
    };
    ($dur:ident, $expr:expr) => {
        {
            let now = std::time::Instant::now();
            let x = $expr;
            $dur += now.elapsed();
            x
        }
    };
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppError(e) => write!(f, "{e}"),
            Self::Status(code, msg) => write!(f, "{code}: {msg}"),
        }
    }
}

pub fn status<T>(code: StatusCode) -> Result<T> {
    status_msg(code, &code.to_string())
}

pub fn status_msg<T>(code: StatusCode, msg: &str) -> Result<T> {
    Err(Error::Status(code, msg.to_string()))
}

pub fn not_found<T>() -> Result<T> {
    status(StatusCode::NOT_FOUND)
}

pub fn bad_req<T>(msg: &str) -> Result<T> {
    status_msg(StatusCode::BAD_REQUEST, msg)
}

pub fn check(ok: bool, msg: &str) -> Result<()> {
    if !ok {
        bad_req(msg)?;
    }
    Ok(())
}

pub fn must<T>(opt: Option<T>) -> Result<T> {
    let Some(x) = opt else {
        return not_found();
    };
    Ok(x)
}

pub type Result<T> = std::result::Result<T, Error>;

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::AppError(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("application error: {e}"),
            ),
            Self::Status(c, msg) => (c, msg),
        }.into_response()
    }
}

impl<E> From<E> for Error
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        Self::AppError(e.into())
    }
}
