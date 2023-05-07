use std::io;

use readyset_adapter::upstream_database::IsFatalError;
use readyset_errors::ReadySetError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    ReadySet(#[from] ReadySetError),

    // TODO(jeb) when we actually get errors from mongo, create a 
    // impl From<Error> for mongo::Error() below (like the mysql and pg impls)
    // #[error(transparent)]
    // MongoDB(#[from] mysql_async::Error),

    #[error(transparent)]
    Io(#[from] io::Error),

}

impl IsFatalError for Error {
    fn is_fatal(&self) -> bool {
        // shit needs to be web-scale, so no foolin' around with "fatal" errors :P
        false
    }
}