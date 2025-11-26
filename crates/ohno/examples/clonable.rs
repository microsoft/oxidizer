//! An example demonstrating a clonable error using the `ohno` crate.

#[ohno::error]
#[derive(Clone)]
#[display("ClonableError: str_field={str_field}, int_field={int_field}")]
struct ClonableError
{
    str_field: String,
    int_field: i32,
}

fn generate_error() -> Result<(), ClonableError> {
    let io_err = std::io::Error::new(std::io::ErrorKind::Other, "I/O failure");
    Err(ClonableError::caused_by(
        "example string",
        42,
        io_err,
    ))
}

fn main() {
    let err = generate_error().unwrap_err();
    let cloned_err = err.clone();

    println!("Original Error: {err}", );
    println!("Cloned Error: {cloned_err}");
}