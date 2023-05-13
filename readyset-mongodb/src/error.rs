use std::io;

use readyset_adapter::upstream_database::IsFatalError;
use readyset_errors::ReadySetError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    ReadySet(#[from] ReadySetError),

    #[error(transparent)]
    MongoDB(#[from] mongodb::error::Error),

    #[error(transparent)]
    Io(#[from] io::Error),

}

impl IsFatalError for Error {
    fn is_fatal(&self) -> bool {
        // shit needs to be web-scale, so no foolin' around with "fatal" errors :P
        false
    }
}