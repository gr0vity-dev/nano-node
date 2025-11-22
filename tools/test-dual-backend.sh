#!/usr/bin/env bash
set -euo pipefail

# --- Visual Helpers ---
# Hide Cursor to stop flickering
cleanup() { tput cnorm; }
trap cleanup EXIT INT TERM
tput civis 

# Function to strip ANSI codes (colors) for the progress bar display
strip_colors() {
    sed 's/\x1b\[[0-9;]*m//g'
}

run_interactive_test() {
    local title="$1"
    local cmd="$2"
    local max_attempts=2
    local log_file
    log_file=$(mktemp)

    for ((attempt=1; attempt<=max_attempts; attempt++)); do
        local attempt_label=""
        if [ "$attempt" -eq 2 ]; then attempt_label="(retry) "; fi

        echo -n "$title ${attempt_label}starting..."
        : > "$log_file"

        # We use a pipe to capture output line-by-line
        # 1. Run Command (capturing stdout and stderr)
        # 2. Tee to logfile (so we save errors)
        # 3. Process line-by-line to update the screen
        
        set +e # Temporarily allow failure so we can capture the exit code
        
        eval "$cmd" 2>&1 | tee "$log_file" | while IFS= read -r line; do
            # Clean line for display (remove colors)
            clean_line=$(echo "$line" | strip_colors)
            
            # Only update screen for interesting lines to reduce flicker
            if [[ "$clean_line" == *"Compiling"* ]] || [[ "$clean_line" == *"test "* ]]; then
                # Cut line to 60 chars max to fit on screen
                short_line=$(echo "$clean_line" | cut -c 1-60)
                # \r moves cursor to start of line, \033[K clears the rest of the line
                printf "\r\033[K%s %s-> %s" "$title" "$attempt_label" "$short_line"
            fi
        done

        # Capture the exit code of the FIRST command in the pipe (cargo test)
        exit_code=${PIPESTATUS[0]}
        set -e # Re-enable strict mode

        if [ $exit_code -eq 0 ]; then
            # --- SUCCESS ---
            # Clear the progress line and print PASS
            printf "\r\033[K%-40s \033[0;32mPASS\033[0m\n" "$title"
            rm "$log_file"
            return 0
        fi

        # Detect compile/build errors; do not retry those
        if strip_colors < "$log_file" | grep -E -q 'error: could not compile|error\[[A-Za-z0-9]+\]:'; then
            printf "\r\033[K%-40s \033[0;31mFAILED (build)\033[0m\n" "$title"
            echo "========================================"
            echo "Captured Output:"
            echo "========================================"
            cat "$log_file"
            echo "========================================"
            rm "$log_file"
            return 1
        fi

        if [ "$attempt" -lt "$max_attempts" ]; then
            printf "\r\033[K%-40s \033[0;33mRETRYING...\033[0m" "$title"
            sleep 1
            continue
        fi

        # --- FAILURE after retries ---
        printf "\r\033[K%-40s \033[0;31mFAILED\033[0m\n" "$title"
        echo "========================================"
        echo "Captured Output:"
        echo "========================================"
        cat "$log_file"
        echo "========================================"
        rm "$log_file"
        return 1
    done
}

EXIT_CODE=0

# 1. LMDB
# We force colors in cargo so the final log dump (if failed) is readable
if ! run_interactive_test "LMDB" "cargo test --workspace --color always $*"; then
    EXIT_CODE=1
fi

# 2. RocksDB
if ! run_interactive_test "RocksDB" "RSNANO_TEST_LEDGER_BACKEND=rocksdb cargo test --workspace --color always $*"; then
    EXIT_CODE=1
fi

if [ $EXIT_CODE -eq 0 ]; then
    echo -e "\n\033[0;32m✔ ALL SYSTEMS GO\033[0m"
    exit 0
else
    echo -e "\n\033[0;31m✘ SOME TESTS FAILED\033[0m"
    exit 1
fi
