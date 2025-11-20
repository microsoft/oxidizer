//! Demonstration of the Reader getter functionality

#[derive(Clone, Debug)]
pub struct Logger {
    name: String,
}

#[derive(Clone, Debug)]
pub struct Database {
    url: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    env: String,
}

#[fundle::bundle]
pub struct AppState {
    logger: Logger,
    config: Config,
    database: Database,
}

fn main() {
    // Build an application with dependencies using the reader getters
    let app = AppState::builder()
        .logger(|_| {
            println!("Creating logger...");
            Logger {
                name: "main_logger".to_string(),
            }
        })
        .config(|reader| {
            // Use the logger() getter to access the Logger
            let logger = reader.logger();
            println!("Creating config with logger: {}", logger.name);
            Config {
                env: format!("{}_env", logger.name),
            }
        })
        .database(|reader| {
            // Use both logger() and config() getters
            let logger = reader.logger();
            let config = reader.config();
            println!(
                "Creating database with logger: {} and config: {}",
                logger.name, config.env
            );
            Database {
                url: format!("postgres://{}@localhost", config.env),
            }
        })
        .build();

    println!("\nFinal app state:");
    println!("  Logger: {}", app.logger.name);
    println!("  Config: {}", app.config.env);
    println!("  Database: {}", app.database.url);

    // Demonstrate manual read() usage
    println!("\n--- Manual read() demonstration ---");
    let builder = AppState::builder()
        .logger(|_| Logger {
            name: "test".to_string(),
        })
        .config(|_| Config {
            env: "dev".to_string(),
        });

    // Convert to read mode
    let reader = builder.read();

    // Access fields using getters
    println!("Logger from reader: {}", reader.logger().name);
    println!("Config from reader: {}", reader.config().env);
}
