#!/bin/bash
# Demo for r/softwarearchitecture
# Usage: asciinema rec demo_arch.cast -c "./demo_arch.sh" --cols 100 --rows 30

SEM="./crates/target/release/sem"

type_cmd() {
    local cmd="$1"
    local i=0
    printf '\n$ '
    while [ $i -lt ${#cmd} ]; do
        printf '%s' "${cmd:$i:1}"
        sleep 0.035
        i=$((i + 1))
    done
    echo
    sleep 0.3
}

pause() {
    sleep "$1"
}

clear

# Scene 1: git diff (the noise)
type_cmd "git diff 64bfe8e~1..64bfe8e --stat"
git diff 64bfe8e~1..64bfe8e --stat
pause 2

type_cmd "git diff 64bfe8e~1..64bfe8e | head -60"
git diff 64bfe8e~1..64bfe8e | head -60
pause 3

# Scene 2: sem diff (the signal)
type_cmd "sem diff --from 64bfe8e~1 --to 64bfe8e"
$SEM diff --from 64bfe8e~1 --to 64bfe8e
pause 4

# Scene 3: impact analysis (what breaks)
type_cmd "sem impact extract_dot_chains"
$SEM impact extract_dot_chains
pause 5
