# JSON Semantic Diff — Behaviour Spec

## What is a JSON entity?

An entity is a single key-value pair anywhere inside a JSON object.
It has:
- A **JSON Pointer path** as its stable identity within the file (e.g. `/scripts/build`)
- A **parent** — the enclosing entity (or none for top-level keys)
- **content** — the raw `"key": value` text, used for content hashing
- **structural_hash** — a hash of the *value only* (key name stripped), used to detect renames

---

## What we extract entities from

| JSON structure | Extract entities? | Recurse into children? |
|---|---|---|
| Root object `{ }` | No (root itself is not an entity) | Yes — all top-level keys become entities |
| Object value `"key": { }` | Yes (the key is an entity) | Yes — recurse into the nested object |
| Array value `"key": [ ]` | Yes (the key is an entity) | **No** — array elements have no stable key name |
| Scalar value `"key": "val"` | Yes | N/A |
| Root is an array `[ ]` | — | File produces no entities at all |

---

## Entity types

| Value type | `entity_type` |
|---|---|
| String, number, boolean, null | `property` |
| Object `{ }` | `object` |
| Array `[ ]` | `array` |

---

## Change detection — all cases

### Top-level scalar

```json
// before               // after
{ "name": "foo" }       { "name": "bar" }
```
→ `name` **Modified**

```json
{ "name": "foo" }       { }
```
→ `name` **Deleted**

```json
{ }                     { "name": "foo" }
```
→ `name` **Added**

```json
{ "timeout": 30 }       { "request_timeout": 30 }
```
→ `request_timeout` **Renamed** (structural_hash matches — same value, different key)

---

### Top-level object

```json
{ "scripts": { "build": "tsc" } }     { "scripts": { "build": "webpack" } }
```
→ `scripts` **Modified**
→ `scripts/build` **Modified**
(Both are reported. Parent content changed because a child changed.)

```json
{ "scripts": { "build": "tsc" } }     { }
```
→ `scripts` **Deleted**
→ `scripts/build` **Deleted**
(When a parent is deleted, all its children are deleted too.)

```json
{ }                                     { "scripts": { "build": "tsc" } }
```
→ `scripts` **Added**
→ `scripts/build` **Added**

```json
{ "config": { "port": 8080 } }        { "settings": { "port": 8080 } }
```
→ `settings` **Renamed** (structural_hash of object value matches)
→ `settings/port` **Added**, `config/port` **Deleted**
(Child entities are not renamed automatically when their parent is renamed — their
IDs are based on their own path, which changed. Phase 2 structural_hash may
recover the match if the value is the same scalar.)

---

### Nested scalar — rename

```json
// before                               // after
{ "scripts": { "build": "tsc" } }      { "scripts": { "compile": "tsc" } }
```
→ `scripts` **Modified** (content changed)
→ `compile` **Renamed** from `build` (structural_hash matches — same value `"tsc"`)

---

### Nested scalar — add/delete

```json
{ "scripts": { "build": "tsc" } }     { "scripts": { "build": "tsc", "test": "jest" } }
```
→ `scripts` **Modified**
→ `scripts/test` **Added**

```json
{ "scripts": { "build": "tsc", "test": "jest" } }     { "scripts": { "build": "tsc" } }
```
→ `scripts` **Modified**
→ `scripts/test` **Deleted**

---

### Array value — always treated as opaque

```json
{ "deps": ["react", "vue"] }           { "deps": ["react", "vue", "lodash"] }
```
→ `deps` **Modified**
(No child entities. Array elements are not tracked.)

```json
{ "deps": [{"name": "react"}] }        { "deps": [{"name": "react-dom"}] }
```
→ `deps` **Modified**
(Array contains objects — we still do not recurse. The whole array is opaque.)

```json
{ "deps": [{"name": "react"}] }        { "dependencies": [{"name": "react"}] }
```
→ `dependencies` **Renamed** from `deps` (structural_hash of array content matches)

---

### Deep nesting (3+ levels)

```json
// before
{
  "jest": {
    "config": {
      "timeout": 5000
    }
  }
}

// after
{
  "jest": {
    "config": {
      "timeout": 10000
    }
  }
}
```
→ `jest` **Modified**
→ `jest/config` **Modified**
→ `jest/config/timeout` **Modified**

---

### Null and empty object values

```json
{ "key": null }                        { "key": "value" }
```
→ `key` **Modified**

```json
{ "key": {} }                          { "key": { "port": 8080 } }
```
→ `key` **Modified**
→ `key/port` **Added**

---

## Structural hash rules (rename detection)

The `structural_hash` is computed from the **value only** — the key name is stripped.
This is what allows rename detection.

| Before | After | content_hash | structural_hash |
|---|---|---|---|
| `"build": "tsc"` | `"compile": "tsc"` | different (key name changed) | **same** → Renamed |
| `"build": "tsc"` | `"build": "webpack"` | different | different → Modified |
| `"config": {"port": 8080}` | `"settings": {"port": 8080}` | different | **same** → Renamed |
| `"config": {"port": 8080}` | `"config": {"port": 9090}` | different | different → Modified |

---

## Double-reporting (expected behaviour)

When a nested entity changes, both the parent and the child are reported.
This is intentional — the parent's content genuinely changed and both levels of
information are useful to the consumer.

```
∆ object     scripts        [modified]    ← parent content changed
∆ property   scripts/build  [modified]    ← specific key that changed
```

This is **not** the same as the array bug, where a spurious child entity was
reported for a key inside an array element that should never have been an entity.

---

## Entity ID format

IDs are stable across runs and unique within a file.

Format: `{file_path}::{entity_type}::{json_pointer}`

Examples:
- `package.json::property::/name`
- `package.json::object::/scripts`
- `package.json::property::/scripts/build`
- `package.json::array::/deps`

Rules:
- The JSON Pointer is always the **full absolute path** from the root (e.g. `/scripts/build`, not just `/build`)
- Key names are JSON Pointer-escaped: `~` → `~0`, `/` → `~1`
- The parent ID is **not** embedded in the child ID — the full pointer is sufficient to uniquely identify any entity

---

## Known limitations

### Parent rename + content change in the same commit

If a parent object is renamed **and** its content changes in the same commit (e.g. a
child is added or removed at the same time), the parent rename cannot be detected.

```json
// before                    // after
{                            {
  "config": {                  "settings": {
    "port": 8080                 "port": 8080,
  }                              "host": "localhost"
}                            }
                             }
```

Expected (ideal): `settings` Renamed, `settings/host` Added
Actual output:
```
- config          [deleted]       ← rename missed
+ settings        [added]         ← rename missed
↻ settings/port   [renamed]       ← port didn't rename; its parent did
+ settings/host   [added]
```

**Why:** The `structural_hash` of the parent is computed from its full value text.
Adding `host` changes that text, so before and after hashes differ and Phase 2
cannot match them. The algorithm cannot distinguish "renamed parent with a new child"
from "deleted old key, added brand-new key that happens to share a child value".

**Accepted behaviour:** Parent rename is reported as Deleted + Added when the parent's
content also changed in the same commit. Child entities with unchanged values may still
be individually matched as Renamed by Phase 2.

---

## Edge cases

| Case | Behaviour |
|---|---|
| Key name contains `/` e.g. `"a/b": 1` | Pointer-escaped to `/a~1b`. Entity ID: `file::property::/a~1b` |
| Key name contains `~` e.g. `"a~b": 1` | Pointer-escaped to `/a~0b` |
| Root document is `[]` | No entities produced |
| Root document is a scalar `"hello"` | No entities produced |
| Empty object `{}` | No entities produced |
| Object with empty nested object `{"key": {}}` | One entity: `key` (type `object`, no children) |
| Object with `null` value `{"key": null}` | One entity: `key` (type `property`) |
