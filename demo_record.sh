#!/bin/bash
# Scripted demo for asciinema recording
# Usage: asciinema rec demo.cast -c "./demo_record.sh"

SEM="./crates/target/release/sem"

type_cmd() {
    local cmd="$1"
    local i=0
    while [ $i -lt ${#cmd} ]; do
        printf '%s' "${cmd:$i:1}"
        sleep 0.04
        i=$((i + 1))
    done
    echo
    sleep 0.3
}

pause() {
    sleep "$1"
}

# Scene 1: sem diff
type_cmd "sem diff --from HEAD~3 --to HEAD --file-exts .rs"
$SEM diff --from HEAD~3 --to HEAD --file-exts .rs
pause 4

# Scene 2: sem entities
type_cmd "sem entities crates/sem-core/src/model/identity.rs"
$SEM entities crates/sem-core/src/model/identity.rs
pause 3

# Scene 3: sem impact
type_cmd "sem impact match_entities"
$SEM impact match_entities
pause 4

# Scene 4: verbose diff (word-level highlights)
type_cmd "sem diff --from HEAD~3 --to HEAD --file-exts .rs -v"
$SEM diff --from HEAD~3 --to HEAD --file-exts .rs -v 2>&1 | head -50
pause 4
