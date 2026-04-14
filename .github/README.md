# CI Runner Images

## Image Selection

| Image | Used For |
|-------|----------|
| `ubuntu-slim` | Lightweight jobs that don't run a full build (spell-check, PR title lint, license headers, external type checks, CodeQL, release publishing, nightly gatekeeper) |
| `ubuntu-latest` | Full builds, tests, static analysis, coverage, mutation testing |
| `ubuntu-24.04-arm` | ARM builds, tests, static analysis, coverage |
| `windows-latest` | Cross-platform builds, tests, static analysis, coverage |
| `windows-11-arm` | ARM builds, tests, static analysis |

## Why `ubuntu-slim`?

`ubuntu-slim` is a 1-vCPU Linux runner with a minimal image, suitable for jobs that
are not CPU- or toolchain-intensive. It starts faster and consumes fewer resources.

- [Standart github runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#standard-github-hosted-runners-for-public-repositories)
- [1-vCPU Linux runner announcement](https://github.blog/changelog/2025-10-28-1-vcpu-linux-runner-now-available-in-github-actions-in-public-preview/)
- [ubuntu-slim image contents](https://github.com/actions/runner-images/blob/main/images/ubuntu-slim/ubuntu-slim-Readme.md)
