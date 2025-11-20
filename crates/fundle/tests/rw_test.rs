//! Test for the new RW type state functionality

#[derive(Clone, Debug, Default)]
pub struct Logger {
    name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Database {
    url: String,
}

#[fundle::bundle]
pub struct AppState {
    logger: Logger,
    database: Database,
}

#[test]
fn test_rw_functionality() {
    // Test that the builder works with the new RW parameter
    let app = AppState::builder()
        .logger(|_x| {
            // The closure now receives a Reader flavor
            Logger { name: "main".to_string() }
        })
        .database(|x| {
            // The closure receives a Reader flavor,
            // and we can access the logger through AsRef
            let logger: &Logger = x.as_ref();
            assert_eq!(logger.name, "main");
            Database { url: "postgresql://localhost".to_string() }
        })
        .build();

    assert_eq!(app.logger.name, "main");
    assert_eq!(app.database.url, "postgresql://localhost");
}

#[test]
fn test_writer_trait() {
    // Test that Writer trait is implemented
    let builder = AppState::builder()
        .logger(|_| Logger { name: "test".to_string() });

    fn takes_writer<T: fundle::Writer>(_: T) {}
    takes_writer(builder);
}

#[test]
fn test_reader_trait() {
    // Test that Reader trait is implemented
    let builder = AppState::builder()
        .logger(|_| Logger { name: "test".to_string() });

    // Convert to read mode
    let reader = builder.read();

    fn takes_reader<T: fundle::Reader>(_: T) {}
    takes_reader(reader);
}

#[test]
fn test_read_toggle() {
    // Test the read() method
    let builder = AppState::builder()
        .logger(|_| Logger { name: "test".to_string() });

    let reader = builder.read();

    // Access the logger through the reader
    let logger: &Logger = reader.as_ref();
    assert_eq!(logger.name, "test");
}

#[test]
fn test_reader_getters() {
    // Test that getter methods work in Read mode
    let builder = AppState::builder()
        .logger(|_| Logger { name: "test_logger".to_string() });

    let reader = builder.read();

    // Use the getter method
    let logger = reader.logger();
    assert_eq!(logger.name, "test_logger");
}

#[test]
fn test_reader_getters_multiple_fields() {
    // Test getters with multiple fields set
    let builder = AppState::builder()
        .logger(|_| Logger { name: "main_logger".to_string() })
        .database(|_| Database { url: "postgres://localhost".to_string() });

    let reader = builder.read();

    // Use getter methods for both fields
    let logger = reader.logger();
    assert_eq!(logger.name, "main_logger");

    let database = reader.database();
    assert_eq!(database.url, "postgres://localhost");
}

#[test]
fn test_reader_getter_in_closure() {
    // Test that getter methods can be used in closures
    let app = AppState::builder()
        .logger(|_| Logger { name: "first".to_string() })
        .database(|x| {
            // Use the getter method instead of AsRef
            let logger = x.logger();
            assert_eq!(logger.name, "first");
            Database { url: format!("db_for_{}", logger.name) }
        })
        .build();

    assert_eq!(app.database.url, "db_for_first");
}
