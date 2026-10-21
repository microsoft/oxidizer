---
name: simple-english
description: |
  Write or rewrite crate documentation and other human-facing documentation
  in plain English. Use short sentences, active voice, simple tenses,
  consistent terms, condition-first commands, and definitions for unfamiliar
  technical terms. Use this skill for rustdoc, READMEs, guides, runbooks,
  procedures, errors, release notes, and API documentation.
license: MIT
compatibility: github-copilot
metadata:
  version: "2.1.1"
  source: https://github.com/AminBlg/SimpleEnglish/tree/main/skills/simple-english
---

# Simple English

Write plain English that a reader outside the project can understand on one
read. These rules come from the SimpleEnglish `simple-english` skill. That
skill applies ideas from ASD-STE100 Simplified Technical English.

## Protect technical content

Do not change code, identifiers, commands, flags, file paths, quoted errors,
product names, or technical facts. Apply these rules to the prose around that
content.

Do not edit generated crate README files. Edit the crate documentation that
generates them. Then run `just anvil-readme --fix`.

## Write procedures

Use these rules for steps, runbooks, and instructions:

1. Use the imperative form.
2. Put a condition before its command.
3. Put one instruction in each sentence.
4. Use no more than 20 words in each sentence.
5. Put a warning before the step that has the risk.
6. State the command first. State the risk second.

Example:

> If the build fails, examine the log. Then correct the first reported error.

## Write descriptions

Use these rules for rustdoc, guides, explanations, and API documentation:

1. Use active voice and simple tenses.
2. Put one new fact in each sentence.
3. Use no more than 25 words in each sentence.
4. Keep one topic in each paragraph.
5. Use no more than six sentences in each paragraph.
6. Define an unfamiliar technical term when you first use it.

## Use consistent words

Use one word for one meaning in the same document. Do not switch between
`configuration`, `settings`, and `options` for the same item.

Use `make sure that` when the reader must establish a state. Use `examine`
when the reader must inspect something for faults.

Use American English spelling. Use complete grammar. Keep articles such as
`a`, `an`, and `the`.

## Use direct language

Use `can`, `will`, and `must`. Do not use `should`, `would`, `may`, `might`,
or `could`.

Do not use contractions, semicolons, or em dashes. Split the sentence or name
the relation with words such as `because`, `but`, or `as a result`.

Delete words that do not add facts. Examples include `simply`, `seamlessly`,
`robust`, `powerful`, `comprehensive`, `crucial`, and `in order to`.

Use common words:

| Do not use | Use |
|---|---|
| leverage, utilize | use |
| prior to | before |
| ensure, verify, confirm | make sure that |
| in the event that | if |
| due to the fact that | because |
| functionality | function or feature |
| enables you to | you can |
| perform | do |
| furthermore, moreover | also |

## Use useful structure

Use formatting only when it shows structure. Do not use bold text as
decoration. Do not use emoji as structure.

Use a vertical list for three or more parallel items or steps. Start each item
with an uppercase letter. Put one instruction in each item.

## Make a final pass

Before you finish, search the changed prose for these items:

- `should`, `would`, `may`, `might`, and `could`
- Semicolons and em dashes
- Contractions
- Sentences with more than 20 words in procedures
- Sentences with more than 25 words in descriptions
- Different names for the same item
- Terms that a new reader cannot understand
- Decorative formatting or words that add no facts

Correct each problem without changing technical content.

## Source and license

This project skill is adapted from the
[SimpleEnglish `simple-english` skill](https://github.com/AminBlg/SimpleEnglish/tree/main/skills/simple-english),
version 2.1.1. The source is copyright 2026 AminBlg and uses the MIT License.
See `LICENSE` in this directory.
