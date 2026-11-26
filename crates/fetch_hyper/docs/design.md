# Hyper transport memory

The application supplies the `HttpBodyBuilder` when it creates a
`HyperTransportBuilder`. That builder supplies the memory provider, clock, and
response-body policies for the transport.

The Hyper transport does not create a general-purpose memory provider. It
copies the supplied body builder into each constructed response path. Clones
of the body builder preserve the provider selected by the application.

Transport option setters configure connections, HTTP versions, and telemetry.
They do not replace the body builder's memory policy.
