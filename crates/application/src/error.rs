use dotlog_domain::DomainError;
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

/// 应用层错误。`code()` 是 `docs/ipc-contract.md` §5 中的稳定错误码。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Domain(#[from] DomainError),

    /// 持久层错误，保留底层错误码（`E_DB` / `E_DB_BUSY` / `E_DB_CORRUPT`）。
    #[error("{message}")]
    Store {
        code: &'static str,
        message: String,
    },
}

impl AppError {
    pub fn store(code: &'static str, message: impl Into<String>) -> Self {
        Self::Store {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Domain(error) => error.code(),
            Self::Store { code, .. } => code,
        }
    }
}
