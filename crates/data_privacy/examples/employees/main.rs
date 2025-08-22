// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shows how redaction is intended to be used to protect sensitive data in telemetry.
//!
//! A given application uses a specific data taxonomy appropriate for its context.
//! Each company or government can have its own taxonomy that defines the types of
//! data the organization recognizes.
//!
//! The redaction framework exists to help prevent sensitive information from being
//! leaked into an application's telemetry. For example, it's generally not a good idea to
//! emit the user's identity in a cloud service's log.
//!
//! Redaction is different from deletion. The redaction framework replaces sensitive data
//! with something else. Often in production, sensitive data is replaced with a hash value.
//! The hash value is not reversible, so the sensitive data cannot be recovered from it.
//! However, having a consistent hash value for a given piece of sensitive data enables correlation
//! across multiple independent log entries in a telemetry system. So for example, although you might
//! now know which user is experiencing problems, you can tell that a specific user is experiencing problems
//! and can track what that user has been doing to get into trouble.
//!
//! In this example, we do the following:
//!
//! * Create a custom taxonomy. Normally, an application would typically use a taxonomy provided by their company to be
//!   used across multiple applications, but here we're doing it stand-alone for the sake of the example.
//!
//! * Initialize a `RedactionEngine`. The engine controls which redaction logic to apply to individual classes of data.
//!   Although this is being hardcoded in this example, the specific redaction algorithm to use for a given data class
//!   should typically be control through external configuration state that the application consumes.
//!
//! * Once the redaction engine is initialized, it is handed over to this application's logging system. This is a made-up
//!   piece of code standing in for whatever logging framework the application uses for logging.
//!
//! * The application does its business and emits logs along the way. The logging system then redacts this data so that the
//!   log output by the application doesn't contain any sensitive information.

mod employee;
mod example_taxonomy;
mod logging;

use std::fs::{File, OpenOptions};
use std::io::BufReader;

use data_privacy::{RedactionEngineBuilder, SimpleRedactor, SimpleRedactorMode};
use employee::Employee;
use example_taxonomy::{ExampleTaxonomy, OrganizationallyIdentifiableInformation, PersonallyIdentifiableInformation};
use logging::{log, set_redaction_engine_for_logging};

fn main() {
    // First step, we create a redaction engine that prescribes how to redact individual data classes.
    // Normally, the specific algorithm to adopt for a given data class would be controlled by external configuration,
    // but for the sake of this example, we hardcode it.
    //
    // If at runtime, an unconfigured data class is encountered, then the data just
    // gets erased, so it is not logged at all, avoiding a potential privacy leak.
    let engine = RedactionEngineBuilder::new()
        .add_class_redactor(
            &ExampleTaxonomy::PersonallyIdentifiableInformation.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Replace('*')),
        )
        .add_class_redactor(
            &ExampleTaxonomy::OrganizationallyIdentifiableInformation.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        )
        .build();

    // now configure the logging system to use the redaction engine
    set_redaction_engine_for_logging(engine);

    // now go run the app's business logic
    app_loop();
}

#[expect(clippy::print_stdout, reason = "this is a demo app, so we print to stdout")]
fn app_loop() {
    let json_path = "employees.json";
    let mut employees: Vec<Employee> = File::open(json_path).map_or_else(
        |_| Vec::new(),
        |file| {
            let reader = BufReader::new(file);
            serde_json::from_reader(reader).unwrap_or_default()
        },
    );

    // pretend some UI collected some data, and we then create an Employee struct to hold this data
    let employee = Employee {
        name: PersonallyIdentifiableInformation::new("John Doe".to_string()),
        address: PersonallyIdentifiableInformation::new("123 Elm Street".to_string()),
        id: OrganizationallyIdentifiableInformation::new("12345-52".to_string()),
        age: 33,
    };

    employees.push(employee.clone());

    let file = OpenOptions::new().write(true).create(true).truncate(true).open(json_path).unwrap();
    serde_json::to_writer_pretty(file, &employees).unwrap();
    println!("Employee added.\n");

    // Here we log the employee creation event. Our little logging framework takes as input a set of name/value pairs that provide
    // a structured log record.
    //
    // For each value, you can control which trait is used to format the value into a string:
    //   `name` - formats the value with the `Display` trait.
    //   `name:?` - formats the value with the `Debug` trait.
    //   `mame:@` - formats the value with the `Display` trait and redacts it.
    log!(event:? = "Employee created",
         name:@ = employee.name,
         address:@ = employee.address,
         employee_id:@ = employee.id,
         age = employee.age);
}
