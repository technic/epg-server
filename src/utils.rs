use iron::prelude::*;
use iron::status;
use std::error::Error as StdError;

pub fn bad_request<E: StdError + Send + 'static>(error: E) -> IronError {
    error_with_status(error, status::BadRequest)
}

pub fn server_error(error: Box<dyn StdError + Send + Sync>) -> IronError {
    box_error_with_status(error, status::InternalServerError)
}

pub fn box_error_with_status(error: Box<dyn StdError + Send>, status: status::Status) -> IronError {
    let m = (status, error.to_string());
    IronError {
        error,
        response: Response::with(m),
    }
}

pub fn error_with_status<E>(error: E, status: status::Status) -> IronError
where
    E: StdError + Send + 'static,
{
    let m = (status, error.to_string());
    IronError::new(error, m)
}
