# RULES.md

This file defines mandatory working rules for this repository. Follow these instructions before making any code, test, refactor, review, or documentation change.

## Additional Rules

- If `RULES.md` and `AGENTS.md` conflict, `AGENTS.md` wins (it carries project-specific constraints like parity requirements); if unclear, ask the user.
- Behavioral parity requirements and documented quirks in `AGENTS.md` are constraints, not technical debt. Do not 'improve' them unless an implementation with better performance can provide the exact same quirks.
- Hot-path optimizations justified by `AGENTS.md`'s performance requirements are allowed. Keep them localized and commented; everywhere else prefer the simplest readable code.
- The `Project-specific carve-outs` and `Rust rules` sections take priority over the other rules in this file.

## Project-specific carve-outs

- Behavioral parity requirements and documented quirks in AGENTS.md are constraints, not technical debt. Do not refactor them away: byte-compatible output, timing semantics, and error counts are acceptance criteria.
- The benchmark hot path (connection state machine, response parsing, engine callbacks) may use localized performance techniques — buffer reuse, avoiding per-request allocation, minimal branching. Keep such code commented with the reason, contained to the hot path, and measured where practical; everywhere else prefer the simplest readable code.

## Rust rules

- Model fallibility with `Result` and typed errors (`thiserror`) in library code; reserve `panic!`/`unwrap` for truly impossible states.
- `expect()` is allowed only at startup/config parsing where failure means the tool cannot run, and the message must state why the invariant holds.
- A panic aborts a running benchmark: no `unwrap()` or panicking indexing on the request/response path.
- `unsafe` is confined to FFI boundaries (mlua, rquickjs), requires a `SAFETY:` comment, and `unsafe_op_in_unsafe_fn` stays denied.
- The `wrkrs-engine` crate is publishable: its public API is semver-protected; breaking changes require a version bump and a changelog entry.
- Feature flags stay additive and minimal; mutually exclusive features are guarded with an explicit `compile_error!` explaining the conflict.
- `just lint` (`cargo fmt --check` + `cargo clippy -- -D warnings`) gates every change; fix or explicitly justify every lint.

## Priority and behavior

- Treat every unqualified rule in this file as `MUST`; treat `Prefer` as `SHOULD`; treat `Do not`, `Avoid`, and `Never` as `MUST NOT` unless the user explicitly overrides it.
- Prefer readability, maintainability, correctness, and safe change over cleverness or speed hacks.
- Optimize for the next human reader.
- When trade-offs exist, choose the option that reduces long-term complexity.
- Never preserve bad structure just because it already exists.
- Apply the Boy Scout Rule: leave touched code cleaner than you found it.

## Core clean code principles

- Write code primarily for humans, not just for execution.
- Keep code simple, direct, and easy to modify.
- Avoid accidental complexity.
- Avoid surprising behavior.
- Prefer explicit intent over implicit magic.
- Prefer local reasoning: a reader should understand code with minimal jumping across files.
- Reduce technical debt instead of moving it around.

## Naming rules

- Use intention-revealing names.
- Names must explain purpose, role, or behavior without requiring extra comments.
- Avoid misleading names, overloaded meanings, and visually confusable identifiers.
- Make distinctions meaningful. Do not create names that differ only cosmetically.
- Use pronounceable, searchable names.
- Avoid abbreviations unless they are established domain or platform standards.
- Avoid encodings in names, including type prefixes, implementation hints, and Hungarian notation.
- Avoid unnecessary context in identifiers.
- Add context through modules, classes, namespaces, or types when that is cleaner than longer names.
- Use one word per concept across the codebase.
- Do not use multiple synonyms for the same operation or concept.
- Do not reuse a familiar word for a different meaning.
- Class, type, and module names should be nouns or noun phrases.
- Function and method names should be verbs or verb phrases.
- Use problem-domain names for domain concepts.
- Use solution-domain names for technical concepts.
- Do not use cute, funny, cryptic, or private-joke names.

## Function rules

- Keep functions small.
- Each function must do one thing.
- A function should have one clear reason to change.
- Keep each function at one level of abstraction.
- Organize code top-down so readers see the high-level story before details.
- Prefer descriptive names over short names.
- Minimize the number of parameters.
- Avoid boolean flag parameters. Split behavior into separate functions instead.
- Avoid output parameters unless language conventions make them necessary.
- Eliminate hidden side effects.
- Separate commands from queries.
- A function that answers a question should not also mutate state.
- Prefer exceptions or explicit result types over ad hoc error codes, according to project language norms.
- Isolate error handling from main logic.
- Eliminate duplication aggressively.
- Prefer straightforward control flow over clever control flow.
- Refactor deep nesting into clearer structure.

## Comment rules

- Do not use comments to compensate for bad naming or bad structure.
- First improve the code, then decide whether a comment is still needed.
- Prefer self-explanatory code.
- Use comments only when they add information the code cannot express well.
- Good comment categories include:
  - legal or licensing requirements
  - non-obvious intent
  - important warnings or constraints
  - rationale for a surprising decision
  - clarification of external behavior or protocol assumptions
- Remove redundant, obsolete, obvious, noisy, and misleading comments.
- Do not narrate the code line by line.
- Keep comments precise and maintain them when code changes.
- Avoid TODO comments unless they are actionable, specific, and necessary.
- Do not use `;` or `—` in comments or documentation.
- Avoid long sentences while preserving the meaning.

## Formatting and structure

- Use consistent formatting across the repository.
- Format code to reveal structure and intent.
- Keep related concepts close together.
- Keep files, classes, and functions reasonably small.
- Use vertical ordering to tell the story from higher level to lower level.
- Use indentation to clarify scope, not to hide complexity.
- Avoid excessive line length when it hurts readability.
- Avoid decorative alignment that is brittle during edits.
- Preserve a layout that supports fast scanning.

## Error handling

- Design error handling deliberately.
- Keep the happy path easy to read.
- Provide enough context in error messages for diagnosis.
- Use error types or exception classes that support caller decisions.
- Do not use `unwrap()`, even when the code is guaranteed not to panic.
- Use `expect()` only at startup with a solid justification.
- Do not return `null` or equivalent absence sentinels when a safer model exists.
- Do not pass `null` or equivalent invalid states unless the API explicitly models that case.
- Prefer exceptions, special cases, empty objects, or explicit optionality according to the codebase's language and conventions.
- Make resource cleanup and shutdown paths correct and visible.

## Tests

- Treat tests as production-quality code.
- Keep tests clean, readable, deterministic, and maintainable.
- A test should communicate one main idea.
- Prefer simple setup and clear assertions.
- Avoid brittle tests coupled to irrelevant implementation details.
- Tests should be fast when possible.
- Tests should be isolated and order-independent.
- Tests should be self-checking.
- Add or update tests for behavior changes, bug fixes, and significant refactors.
- Do not ship code changes without proportionate validation.
- When fixing a bug, add a test that would have caught it, when feasible.

## Concurrency and async work

- Do not introduce concurrency unless it provides a real benefit.
- Prefer simpler sequential code when it is sufficient.
- Minimize shared mutable state.
- Prefer immutability, message passing, or clear ownership boundaries.
- Keep synchronized or locked sections as small as possible.
- Be explicit about shutdown, cancellation, timeouts, and cleanup.
- Test concurrent behavior carefully where it matters.
- Know the execution model before changing concurrent code.
- Avoid dependencies between synchronized methods.
- Get non-concurrent behavior correct before adding threading.
- Make threaded code pluggable and tunable when its policy or concurrency level may vary.
- Run concurrency-sensitive tests under varied thread counts, schedules, and platforms where practical.
- Treat spurious failures as possible concurrency defects until evidence says otherwise.

## Review checklist

Before finishing, verify all of the following:

- Names reveal intent.
- Functions are small and focused.
- Classes and modules have clear responsibilities.
- Comments are necessary and accurate.
- Error handling is explicit and useful.
- Duplication was removed where reasonable.
- Tests cover the changed behavior appropriately.
- The code reads cleanly from top to bottom.
- The design is simpler or at least not more complex than before.
- The change follows existing project conventions.

## Output Expectations

When making changes:

- Briefly explain what changed.
- State what tests or checks were run.
- Call out any unresolved risk, assumption, or trade-off.
- If a requested change conflicts with these rules, follow the user request but mention the conflict explicitly.

## Hard rules

- Do not introduce misleading names.
- Do not keep duplicated logic without a strong reason.
- Do not add comments where better code would remove the need.
- Do not mix querying with mutation without a strong reason.
- Do not silently broaden scope beyond the requested task.
- Do not leave touched code less readable than before.

## Git Commit Rules

### 1. Commit Message Structure

Every commit message must follow the Conventional Commits format:

```txt
<type>(<scope>(optional)): <description>
```

#### Types

- **feat**: A new feature for the user
- **fix**: A bug fix
- **docs**: Documentation-only changes
- **style**: Formatting changes that do not affect code execution (white-space, semi-colons, etc.)
- **refactor**: Code changes that neither fix a bug nor add a feature
- **perf**: Code changes that improve performance
- **test**: Adding or correcting existing tests
- **chore**: Changes to the build process, tool configs, or dependencies

#### Scopes

Use lowercase area names as scopes: `engine`, `core`, `cli`, `stats`, `net`, `tls`, `script`, `conformance`, `docs`, `ci`, `deps`, `release`.

---

### 2. Subject Line Formatting

- **Use the imperative mood**: Write "add feature" instead of "added feature" or "adds feature".
- **Capitalization**: Start the summary with a lowercase letter.
- **No trailing period**: Do not end the subject line with a period.
- **Character limit**: Keep the subject line to **50 characters or fewer** (hard limit: 72 characters).

---

### 3. Body & Footers

- **Separate body with a blank line**: Always leave a blank line between the subject line and the body.
- **Explain the "why" and "what"**: Focus on the reason for the change, not just how it was implemented.
- **Wrap lines**: Limit lines in the body to **72 characters**.
- **Reference issues**: Close or link related tickets in the footer (e.g., `Fixes #123` or `Closes PROJ-456`).
- **Breaking Changes**: Highlight breaking changes in the footer using `BREAKING CHANGE: <description>` or a `!` after the type/scope (e.g., `feat(api)!: change user endpoint payload`). (Throughout the migration, this rule can be ignored because every commit will somewhat be a breaking change for the repo.)

---

### 4. Git Hygiene & Best Practices

- **Atomic Commits**: Each commit should represent a single logical change. Do not mix unrelated refactors and feature additions in one commit.
- **Never commit broken code**: The repository build and test suite must pass on every individual commit.
- **No generated files**: Do not commit build artifacts, node_modules, secrets, or temporary files. Ensure `.gitignore` is up to date.
- **Clean up local history**: Rebase and squash local WIP (work in progress) commits before opening or requesting a review on a Pull Request.
