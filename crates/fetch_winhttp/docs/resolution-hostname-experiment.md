# WinHTTP resolution-hostname experiment

This experiment determines whether WinHTTP can authenticate one logical DNS identity while
connecting to another host. It exercises `WINHTTP_OPTION_RESOLUTION_HOSTNAME` directly against a
local TLS server and does not change the machine certificate store or resolver configuration.

The baseline positive case uses three observable values:

- the WinHTTP server name is `winhttp-resolution.invalid`;
- the resolution hostname is `localhost`;
- the server listens only on a loopback address.

The server certificate contains only `winhttp-resolution.invalid`. WinHTTP ignores the certificate's
unknown issuer for this isolated experiment, but hostname validation remains enabled. A successful
request therefore demonstrates that the resolution override reached loopback while TLS validation
used the logical server name. The positive case requires HTTP/2, and the server independently
records the ClientHello SNI and HTTP/2 `:authority`.

A second positive case adds `Host: localhost:<port>` with
`WinHttpAddRequestHeaders(WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE)`. Microsoft
documents this API as providing detailed control over the exact request and permits adding or
replacing well-formed headers. The documentation does not explicitly describe how `Host` is
translated for HTTP/2, so the server records the resulting `:authority`.

A negative control connects as `localhost` to the same kind of server certificate. It must fail
with hostname validation enabled. This distinguishes the intended behavior from accidentally
disabling all certificate validation.

Run the probe on Windows:

```text
cargo +1.93.0 run -p fetch_winhttp --example resolution_hostname
```

The probe passes only when:

1. the positive request reaches the loopback server;
2. the server observes `winhttp-resolution.invalid` as SNI;
3. WinHTTP negotiates HTTP/2;
4. the server observes the logical name in the HTTP/2 `:authority`;
5. the authority-override request retains the logical SNI but emits `localhost:<port>` as
   HTTP/2 `:authority`;
6. WinHTTP returns successful responses; and
7. the negative control fails before sending an HTTP request.

`WINHTTP_OPTION_RESOLUTION_HOSTNAME` requires Windows 10 version 21H1 or later. An unsupported
system reports `ERROR_WINHTTP_INVALID_OPTION`; that result means this mechanism cannot be used on
that host rather than that the TLS behavior failed.

## Observed result

The probe passes on a supported Windows host:

```text
positive: status=200, protocol=1, SNI=winhttp-resolution.invalid, \
:authority=winhttp-resolution.invalid:<loopback-port>
authority override: status=200, protocol=1, SNI=winhttp-resolution.invalid, \
:authority=localhost:<loopback-port>
negative: WinHTTP error=12175, SNI=localhost, HTTP request sent=false
PASS: WinHTTP resolved winhttp-resolution.invalid through localhost while using \
winhttp-resolution.invalid for SNI and certificate hostname validation. A replacement Host header \
independently controlled the HTTP/2 :authority.
```

`protocol=1` is `WINHTTP_PROTOCOL_FLAG_HTTP2`. The negative result is
`ERROR_WINHTTP_SECURE_FAILURE`; the server observes the `localhost` SNI but no HTTP request,
demonstrating that ignoring the unknown issuer did not disable hostname validation. The authority
override demonstrates that current WinHTTP translates an application-supplied `Host` header into
HTTP/2 `:authority` without changing SNI or certificate validation. This translation is verified
behavior rather than an explicit compatibility guarantee in the Microsoft documentation and
therefore requires a retained integration test.

## Documented surface

The WinHTTP request and option documentation provides no dedicated SNI or HTTP authority setter:

- [`WinHttpConnect`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpconnect)
  accepts the logical server name and port.
- [`WinHttpOpenRequest`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpopenrequest)
  accepts only the resource path beneath that connection.
- [`WINHTTP_OPTION_RESOLUTION_HOSTNAME`](https://learn.microsoft.com/windows/win32/winhttp/option-flags#winhttp_option_resolution_hostname)
  changes only the hostname used for DNS resolution.
- [`WINHTTP_OPTION_URL`](https://learn.microsoft.com/windows/win32/winhttp/option-flags#winhttp_option_url)
  retrieves the effective URL and is not settable.
- [`WinHttpAddRequestHeaders`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpaddrequestheaders)
  and
  [`WinHttpAddRequestHeadersEx`](https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpaddrequestheadersex)
  add or replace ordinary request headers. Neither page states how `Host` maps to HTTP/2
  `:authority`.

The documented callback surface can report secure failures and expose a server certificate
context, and security flags can selectively disable built-in checks. It does not provide a
pre-disclosure certificate-validation callback that can substitute an arbitrary DNS identity.
Consequently the exact-name design relies on the documented logical connection and resolution
controls, plus the integration-tested `Host` translation for preserving an independently chosen
HTTP authority.
