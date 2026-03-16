# Scripts

## Dependency Auditing

### `audit-dependencies.sh`

Comprehensive dependency auditing script that checks for:
- **Security vulnerabilities** using `cargo-audit` (RustSec advisory database)
- **Outdated dependencies** using `cargo-outdated`

**Usage:**
```bash
./scripts/audit-dependencies.sh
```

**Prerequisites:**
```bash
# Install audit tools (one-time setup)
cargo install cargo-audit --locked
cargo install cargo-outdated
```

**What it does:**
1. Checks if `cargo-audit` is installed and runs security vulnerability scan
2. Checks if `cargo-outdated` is installed and reports outdated dependencies
3. Provides helpful error messages and installation instructions if tools are missing

**Integration:**
- This script is automatically run in CI (GitHub Actions)
- Security vulnerabilities will fail the CI build
- Outdated dependencies are reported but don't fail the build

## generate_bpmn_examples

Generates a markdown file containing BPMN XML examples from tests and their corresponding CLTS DSL translations.

### Usage

```bash
# Generate default output file (bpmn_examples_output.md)
cargo run --bin generate_bpmn_examples

# Generate to a custom file
cargo run --bin generate_bpmn_examples output.md
```

### Output Format

The script generates a markdown file with:

1. **BPMN XML** - The original XML content from test examples
2. **CLTS DSL Translation** - The translated CLTS DSL context
3. **JSON Output** - JSON representation containing:
   - `context_name`: Name of the generated context
   - `dsl_source`: The full CLTS DSL source code
   - `automata_count`: Number of automata in the context
   - `sidecars_count`: Number of sidecar documents

### Example Output

```markdown
## Example 1: test_parse_minimal_bpmn_xml

### BPMN XML

```xml
<?xml version="1.0" encoding="UTF-8"?>
...
```

### CLTS DSL Translation

```clts
context SimpleProcess {
    ...
}
```

### JSON Output

```json
{
  "context_name": "SimpleProcess",
  "dsl_source": "context SimpleProcess {...}",
  "automata_count": 1,
  "sidecars_count": 0
}
```
```

