# Version rules

Change-type strength is:

```text
none < patch < nonbreaking < breaking
```

Given `major.minor.patch`:

| Current version | breaking | nonbreaking | patch |
|---|---|---|---|
| `x.y.z`, `x >= 1` | `(x+1).0.0` | `x.(y+1).0` | `x.y.(z+1)` |
| `0.y.z`, `y >= 1` | `0.(y+1).0` | `0.y.(z+1)` | `0.y.(z+1)` |
| `0.0.z` | `0.0.(z+1)` | `0.0.(z+1)` | `0.0.(z+1)` |

These rules are intentional:

- On `0.y.z`, nonbreaking and patch produce the same number but retain distinct
  classifications.
- Every `0.0.z` transition is breaking under Cargo compatibility. If a dependent
  exposes that crate, even a patch-classified `0.0.z` release gives the dependent
  a breaking cascade floor.
- Explicit pins retain their exact prerelease/build spelling, but comparisons use
  SemVer precedence and ignore build metadata.
- Generated non-pinned target versions are clean three-component SemVer values.
