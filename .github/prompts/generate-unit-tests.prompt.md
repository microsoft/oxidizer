---
mode: 'agent'
description: 'Generate idiomatic Rust unit tests'
---

# Generate Idiomatic Rust Unit Tests

You are an expert Rust test engineer who writes concise, readable, and
maintainable tests.

## TASK

Generate a comprehensive **Rust** test suite with high coverage for the
specified item (function, impl block, struct, etc.). Focus on **correctness
over 100% coverage** - test all meaningful behaviors and edge cases.

**Note**: If the item already has existing unit tests, analyze them first to
understand the current test structure and patterns. Follow the existing naming
conventions, test organization, and style. Only add tests for functionality
that is not already covered by the existing test suite.

## RULES

### Test Philosophy

- **Succinct over exhaustive**: Write the minimum number of tests that provide
  confidence in correctness.
- **Behavior over implementation**: Test what the code does, not how it does it.
- **Readable over clever**: Prefer clear, simple tests over complex
  parametrized ones.

### Assertions

- Use standard macros: `assert!`, `assert_eq!`, `assert_ne!`,
  `assert_matches!`.
- For compile-time checks, use `static_assertions` crate (`const_assert!`,
  `assert_impl_all!`, etc.).
- **Avoid** verbose assertion libraries; standard macros are sufficient.
- **Prefer** `unwrap()` and `unwrap_err()` over `assert!(result.is_ok())` and
  `assert!(result.is_err())` for cleaner code and better error messages.

### Code Style

- **Never** include explanatory comments in tests unless absolutely necessary -
  the test name and code should be self-documenting.
- Use the most direct assertion method available (e.g., `result.unwrap_err()`
  instead of `assert!(result.is_err())`).
- Keep test code minimal and focused on the single behavior being tested.

### Test Doubles

- **Prefer real objects** over mocks when practical (faster, more reliable).
- Use crate-provided fakes when available.
- Use **mockall** only when necessary for external dependencies.
- For panic testing in memory-related code, consider using custom panic
  assertion macros if available in the codebase.

### Naming

Follow `test_<function>_<condition>_<expectation>` pattern consistently:
- `test_new_with_valid_input_creates_instance`
- `test_parse_with_empty_string_returns_error`
- `test_send_when_disconnected_panics`

**Important**: Always include the condition part, even for simple cases:

- ✅ `test_new_with_no_input_creates_empty_collection`
- ❌ `test_new_creates_empty_collection` (missing condition)

### Structure

Keep tests flat and readable. Always include `use super::*;` in test modules:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_with_condition_produces_expectation() {
        // Simple setup
        let input = create_test_input();

        // Single action
        let result = function_under_test(input);

        // Clear assertions
        assert_eq!(result, expected);
    }
}
```

Use Arrange/Act/Assert comments **only** for complex tests (>10 lines).

### Parametrized Tests

Use `#[rstest]` **sparingly** - only when testing multiple similar inputs:

```rust
#[rstest]
#[case(0, false)]
#[case(1, true)]
#[case(100, true)]
fn test_is_positive(#[case] input: i32, #[case] expected: bool) {
    assert_eq!(is_positive(input), expected);
}
```

Prefer separate test functions for distinct behaviors.

### Error Testing

- Use `unwrap_err()` instead of `assert!(result.is_err())` for better error
  messages when tests fail.
- Use `unwrap()` when expecting success, rather than `assert!(result.is_ok())`.
- Use `#[should_panic(expected = "specific message")]` when panic message is
  part of the API contract.
- Test error types with `assert_matches!(err, SpecificError::Variant)`.
- **Never** include explanatory comments that restate what the code already
  shows - let the test code speak for itself.

### Async & Concurrency

- Use `async` functions and `await` syntax for clarity.

### Coverage Guidelines

Test these scenarios when applicable:

- **Happy path**: Normal, expected usage
- **Edge cases**: Empty inputs, boundary values, None/Some variations
- **Error conditions**: Invalid inputs, resource exhaustion
- **Async behavior**: If applicable, test different async outcomes

**Avoid** testing:

- Private implementation details
- Standard library behavior
- Trivial getters/setters without logic

### Examples to Follow

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_no_input_creates_empty_collection() {
        let collection = Collection::new();
        assert!(collection.is_empty());
    }

    #[test]
    fn test_add_with_item_increases_size() {
        let mut collection = Collection::new();
        collection.add(42);
        assert_eq!(collection.len(), 1);
    }

    #[test]
    fn test_parse_with_invalid_input_returns_error() {
        let result = parse("invalid");
        result.unwrap_err();
    }

    #[test]
    fn test_ensure_success_with_error_status_fails() {
        let status = StatusCode::IM_A_TEAPOT;
        status.ensure_success().unwrap_err();
    }
}
```

### Anti-Patterns to Avoid

```rust
// ❌ Too granular - testing implementation details
#[test]
fn test_internal_counter_incremented_exactly_once() { ... }

// ❌ Over-parameterized - hard to understand failures
#[rstest]
#[case(input1, input2, input3, expected1, expected2, expected3)]
fn test_complex_scenario(...) { ... }

// ❌ Testing standard library
#[test]
fn test_vec_push_increases_length() { ... }

// ❌ Unnecessary comments explaining obvious test behavior
#[test]
fn test_statuscode_ensure_success_with_custom_returns_ok() {
    let status = StatusCode::IM_A_TEAPOT;
    let result = status.ensure_success();
    // IM_A_TEAPOT is in 400 range, so should fail
    assert!(result.is_err());
}

// ❌ Verbose assertion patterns
#[test]
fn test_parse_invalid_returns_error() {
    let result = parse("invalid");
    assert!(result.is_err());  // Use unwrap_err() instead
}

// ✅ Good - tests behavior concisely without explanatory comments
#[test]
fn test_process_increments_user_score() { ... }

// ✅ Good - clear assertion that fails with helpful message
#[test]
fn test_ensure_success_with_client_error_fails() {
    let status = StatusCode::IM_A_TEAPOT;
    status.ensure_success().unwrap_err();
}
```

### Post Processing

After generating tests and verifying they pass:

- Fix the clippy issues by running `cargo clippy --fix --allow-dirty`.
- Format the code using `rustfmt` to ensure consistent style.

