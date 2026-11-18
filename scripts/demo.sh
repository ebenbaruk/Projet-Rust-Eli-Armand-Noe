#!/usr/bin/env bash
# Visual demo of CrashLend in the terminal.
#
# Starts the server, opens three loans, shows how the API refuses invalid
# operations, replays Black Thursday with the liquidation bots, runs the stress
# test and saves the state. Every step goes through the HTTP API, like any
# other client would.
#
# Run it from anywhere: ./scripts/demo.sh
#   --auto   run every step without waiting for Enter
# Requires cargo, curl and jq. Port 3000 must be free.

set -euo pipefail

cd "$(dirname "$0")/.."

BASE_URL="http://127.0.0.1:3000"
SCENARIO="black-thursday"
STRESS_SCENARIOS=10000
BAR_WIDTH=30

AUTO=false
if [ "${1:-}" = "--auto" ] || [ ! -t 0 ]; then
    AUTO=true
fi

# Colors and live lines only when the output is a terminal.
if [ -t 1 ]; then
    LIVE=true
    BOLD=$'\033[1m'
    DIM=$'\033[2m'
    RED=$'\033[31m'
    GREEN=$'\033[32m'
    YELLOW=$'\033[33m'
    CYAN=$'\033[36m'
    RESET=$'\033[0m'
else
    LIVE=false
    BOLD=""
    DIM=""
    RED=""
    GREEN=""
    YELLOW=""
    CYAN=""
    RESET=""
fi

TMP_DIR=$(mktemp -d)
SERVER_PID=""
STEP=0
LIQUIDATIONS_SHOWN=0

# Stops the server started by the demo and removes the temporary files.
cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        # Waiting for it hides the "Terminated" notice of the shell.
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Formatting. The API sends integers (cents, gwei, basis points), so the
# conversions below use integer arithmetic only, like the protocol itself.
# ---------------------------------------------------------------------------

# Adds thousands separators to an integer: 1400000 -> 1,400,000.
group_digits() {
    echo "$1" | sed -E -e ':a' -e 's/^([0-9]+)([0-9]{3})/\1,\2/' -e 'ta'
}

# USDC cents -> "1,400.00 USDC".
usdc() {
    printf '%s.%02d USDC' "$(group_digits $(($1 / 100)))" $(($1 % 100))
}

# gwei -> "1.0000 ETH".
eth() {
    printf '%d.%04d ETH' $(($1 / 1000000000)) $(($1 % 1000000000 / 100000))
}

# Basis points -> "48.02%".
percent() {
    printf '%d.%02d%%' $(($1 / 100)) $(($1 % 100))
}

# Health factor scaled by 10 000 -> "1.14". The API sends null without debt.
health() {
    if [ "$1" = "null" ]; then
        echo "no debt"
    else
        printf '%d.%02d' $(($1 / 10000)) $(($1 % 10000 / 100))
    fi
}

# Colors a guardian mode: Normal, Alert or Emergency.
color_mode() {
    case "$1" in
        Normal) echo "${GREEN}$1${RESET}" ;;
        Alert) echo "${YELLOW}$1${RESET}" ;;
        *) echo "${RED}$1${RESET}" ;;
    esac
}

# ---------------------------------------------------------------------------
# Layout and HTTP helpers.
# ---------------------------------------------------------------------------

# Prints the title of a step and one line about what it shows.
step() {
    STEP=$((STEP + 1))
    echo
    echo "${BOLD}${CYAN}== $STEP. $1${RESET}"
    echo "${DIM}$2${RESET}"
    echo
}

# Erases the live line before printing a result. Does nothing without a terminal.
clear_line() {
    if [ "$LIVE" = true ]; then
        printf '\r\033[K'
    fi
}

# Waits for Enter, so the presenter can comment on each step.
pause() {
    if [ "$AUTO" = false ]; then
        echo
        read -r -p "${DIM}Press Enter to continue...${RESET}" _
    fi
}

# Sends a request to the API, writes the body to $TMP_DIR/body and prints the
# HTTP status. Usage: call METHOD PATH [JSON_BODY]
call() {
    local args=(-s -o "$TMP_DIR/body" -w '%{http_code}' -X "$1" "$BASE_URL$2")
    if [ -n "${3:-}" ]; then
        args+=(-H 'Content-Type: application/json' -d "$3")
    fi
    curl "${args[@]}"
}

# Prints the JSON body of a GET request.
get() {
    curl -s "$BASE_URL$1"
}

# Runs an operation that must succeed and prints one line about it.
# Usage: expect_ok LABEL METHOD PATH [JSON_BODY]
expect_ok() {
    local status
    status=$(call "$2" "$3" "${4:-}")
    if [ "$status" != "200" ]; then
        echo "  ${RED}[$status]${RESET} $1: $(cat "$TMP_DIR/body")"
        exit 1
    fi
    echo "  ${GREEN}[ok]${RESET} $1"
}

# Runs an operation that must fail and prints its status and error message.
# Usage: expect_error LABEL METHOD PATH [JSON_BODY]
expect_error() {
    local status message
    status=$(call "$2" "$3" "${4:-}")
    # Our errors are JSON; a body refused by serde comes back as plain text.
    message=$(jq -r '.error' "$TMP_DIR/body" 2>/dev/null || cat "$TMP_DIR/body")
    printf "  ${YELLOW}%s${RESET}  %-34s %s\n" "$status" "$1" "$message"
}

# ---------------------------------------------------------------------------
# Steps of the demo.
# ---------------------------------------------------------------------------

check_requirements() {
    for tool in cargo curl jq; do
        if ! command -v "$tool" > /dev/null; then
            echo "${RED}$tool is required to run the demo.${RESET}"
            exit 1
        fi
    done
}

banner() {
    echo
    echo "${BOLD}CrashLend${RESET}"
    echo "A simplified lending protocol, replayed through real ETH crashes."
}

start_server() {
    step "Start the server" "Release build, so the stress test takes seconds instead of a minute."
    if curl -s "$BASE_URL/price" > /dev/null; then
        echo "${RED}A server already runs on port 3000. Stop it first: pkill crashlend${RESET}"
        exit 1
    fi
    cargo build --release --quiet
    ./target/release/crashlend > "$TMP_DIR/server.log" 2>&1 &
    SERVER_PID=$!

    # Wait until the server answers, at most 10 seconds.
    for _ in $(seq 1 100); do
        if curl -s "$BASE_URL/price" > /dev/null; then
            echo "  ${GREEN}[ok]${RESET} listening on $BASE_URL"
            return
        fi
        sleep 0.1
    done
    echo "${RED}The server did not start:${RESET}"
    cat "$TMP_DIR/server.log"
    exit 1
}

show_pool() {
    local cash debt utilization rate price
    IFS=$'\t' read -r cash debt utilization rate price < <(
        get /pool | jq -r '[.cash, .total_debt, .utilization, .borrow_rate, .price] | @tsv'
    )
    echo "  ${BOLD}Pool${RESET}"
    printf "  %-14s %s\n" "cash" "$(usdc "$cash")"
    printf "  %-14s %s\n" "total debt" "$(usdc "$debt")"
    printf "  %-14s %s\n" "utilization" "$(percent "$utilization")"
    printf "  %-14s %s a year\n" "borrow rate" "$(percent "$rate")"
    printf "  %-14s %s\n" "ETH price" "$(usdc "$price")"
}

show_positions() {
    echo "  ${BOLD}Positions${RESET}"
    printf "  ${DIM}%-8s %-12s %-16s %-14s %s${RESET}\n" user collateral debt "health factor" liquidatable
    get /positions |
        jq -r '.[] | [.user, .collateral, .debt, (.health_factor // "null"), .liquidatable] | @tsv' |
        while IFS=$'\t' read -r user collateral debt factor liquidatable; do
            printf "  %-8s %-12s %-16s %-14s %s\n" \
                "$user" "$(eth "$collateral")" "$(usdc "$debt")" "$(health "$factor")" "$liquidatable"
        done
}

open_loans() {
    step "Open three loans" \
        "Alice supplies USDC. Bob, Carol and Dave each deposit 1 ETH and borrow against it (protocol.rs, api/lending.rs)."
    expect_ok "alice supplies $(usdc 10000000)" POST /supply '{"user":"alice","amount":10000000}'
    for loan in bob:140000 carol:120000 dave:100000; do
        local user=${loan%%:*}
        local amount=${loan##*:}
        expect_ok "$user deposits $(eth 1000000000)" POST /deposit "{\"user\":\"$user\",\"amount\":1000000000}"
        expect_ok "$user borrows $(usdc "$amount")" POST /borrow "{\"user\":\"$user\",\"amount\":$amount}"
    done
    echo
    show_pool
    echo
    show_positions
}

show_errors() {
    step "Refused operations" \
        "Each LendingError becomes an HTTP status and a JSON body (error.rs, api/mod.rs). A malformed body is refused by serde first."
    expect_error "bob borrows 10,000,000 USDC" POST /borrow '{"user":"bob","amount":1000000000}'
    expect_error "zoe borrows without a position" POST /borrow '{"user":"zoe","amount":100}'
    expect_error "bob borrows nothing" POST /borrow '{"user":"bob","amount":0}'
    expect_error "bob sends text as an amount" POST /borrow '{"user":"bob","amount":"abc"}'
    expect_error "replay of an unknown crash" POST /replay/unknown
}

# Prints the liquidations made since the last call, one line each.
print_new_liquidations() {
    get /liquidations |
        jq -r --argjson from "$LIQUIDATIONS_SHOWN" '.[$from:][] | [.minute, .bot, .user, .repaid, .seized] | @tsv' \
            > "$TMP_DIR/new"
    while IFS=$'\t' read -r minute bot user repaid seized; do
        clear_line
        printf "  ${RED}liquidation${RESET} %02d:%02d  bot %-8s %-6s repays %-15s seizes %s\n" \
            $((minute / 60)) $((minute % 60)) "$bot" "$user" "$(usdc "$repaid")" "$(eth "$seized")"
        LIQUIDATIONS_SHOWN=$((LIQUIDATIONS_SHOWN + 1))
    done < "$TMP_DIR/new"
}

# Redraws the live line: ETH price as a bar, drop since the start, guardian mode.
draw_status() {
    local start_price=$1 elapsed=$2 price mode drop filled bar="" i
    if [ "$LIVE" = false ]; then
        return
    fi
    price=$(get /price | jq '.price')
    mode=$(get /risk | jq -r '.mode')

    drop=$(((start_price - price) * 10000 / start_price))
    if [ "$drop" -lt 0 ]; then
        drop=0
    fi
    filled=$((price * BAR_WIDTH / start_price))
    for ((i = 0; i < BAR_WIDTH; i++)); do
        if [ "$i" -lt "$filled" ]; then
            bar="$bar#"
        else
            bar="$bar."
        fi
    done

    printf "\r\033[K  ETH %-15s [%s] -%-7s guardian %-20s %2ss" \
        "$(usdc "$price")" "$bar" "$(percent "$drop")" "$(color_mode "$mode")" "$elapsed"
}

show_report() {
    local report bad_debt
    report=$(get /report)
    bad_debt=$(echo "$report" | jq '.bad_debt')

    echo "  ${BOLD}Report${RESET}"
    printf "  %-16s %s\n" "start price" "$(usdc "$(echo "$report" | jq '.start_price')")"
    printf "  %-16s %s\n" "lowest price" "$(usdc "$(echo "$report" | jq '.lowest_price')")"
    printf "  %-16s -%s\n" "maximum drop" "$(percent "$(echo "$report" | jq '.max_drop')")"
    printf "  %-16s %s\n" "liquidations" "$(echo "$report" | jq '.liquidations')"
    printf "  %-16s %s\n" "debt repaid" "$(usdc "$(echo "$report" | jq '.repaid')")"
    if [ "$bad_debt" = "0" ]; then
        printf "  %-16s ${GREEN}%s, the pool survived${RESET}\n" "bad debt" "$(usdc 0)"
    else
        printf "  %-16s ${RED}%s${RESET}\n" "bad debt" "$(usdc "$bad_debt")"
    fi

    echo
    printf "  ${DIM}%-10s %-14s %-8s %s${RESET}\n" bot liquidations missed repaid
    echo "$report" | jq -r '.bots[] | [.name, .liquidations, .missed, .repaid] | @tsv' |
        while IFS=$'\t' read -r name liquidations missed repaid; do
            printf "  %-10s %-14s %-8s %s\n" "$name" "$liquidations" "$missed" "$(usdc "$repaid")"
        done
}

replay_crash() {
    step "Replay Black Thursday" \
        "ETH prices of 12 March 2020, one per minute, rescaled to start at the pool price. The replay and three bots run as tokio tasks (replay.rs, bots.rs) while the guardian watches the drop (guardian.rs)."
    local minutes real_start start_price status started
    IFS=$'\t' read -r minutes real_start < <(
        get /scenarios | jq -r --arg id "$SCENARIO" '.[] | select(.id == $id) | [.minutes, .start_price] | @tsv'
    )
    start_price=$(get /price | jq '.price')
    echo "  $(group_digits "$minutes") minutes, real price at 00:00 UTC: $(usdc "$real_start"), replayed from $(usdc "$start_price")"

    status=$(call POST "/replay/$SCENARIO")
    echo "  POST /replay/$SCENARIO answered $status: the replay runs in the background"
    status=$(call POST "/replay/$SCENARIO")
    echo "  a second POST answers $status: one replay at a time"
    echo

    started=$SECONDS
    while [ "$(get /report)" = "null" ]; do
        print_new_liquidations
        draw_status "$start_price" $((SECONDS - started))
        sleep 0.2
    done
    print_new_liquidations
    clear_line
    echo "  replay finished in $((SECONDS - started)) s"
    echo
    show_report
}

run_stress_test() {
    step "Stress test" \
        "$(group_digits $STRESS_SCENARIOS) random crashes, each replayed with and without the guardian, spread over every core with std::thread::scope inside spawn_blocking (stress_test.rs, api/risk.rs)."
    curl -s -X POST "$BASE_URL/stress-test" -H 'Content-Type: application/json' \
        -d "{\"scenarios\":$STRESS_SCENARIOS}" > "$TMP_DIR/stress.json" &
    local pid=$! started=$SECONDS frames="|/-\\" i=0
    while kill -0 "$pid" 2>/dev/null; do
        if [ "$LIVE" = true ]; then
            printf "\r\033[K  %s running for %ss" "${frames:i % 4:1}" $((SECONDS - started))
        fi
        i=$((i + 1))
        sleep 0.2
    done
    wait "$pid"
    clear_line

    local r="$TMP_DIR/stress.json"
    local survival_without survival_with debt_without debt_with threads ms
    survival_without=$(jq '.survival_rate_without_guardian * 10000 | round' "$r")
    survival_with=$(jq '.survival_rate_with_guardian * 10000 | round' "$r")
    debt_without=$(jq '.average_bad_debt_without_guardian' "$r")
    debt_with=$(jq '.average_bad_debt_with_guardian' "$r")
    threads=$(jq '.threads' "$r")
    ms=$(jq '.duration_ms' "$r")

    printf "  ${DIM}%-20s %-20s %s${RESET}\n" "" "without guardian" "with guardian"
    printf "  %-20s %-20s %s\n" "survival rate" "$(percent "$survival_without")" "$(percent "$survival_with")"
    printf "  %-20s %-20s %s\n" "average bad debt" "$(usdc "$debt_without")" "$(usdc "$debt_with")"
    echo
    printf "  %s threads, %d.%d s\n" "$threads" $((ms / 1000)) $((ms % 1000 / 100))
}

save_state() {
    step "Save and load the state" \
        "The whole Protocol is written to state.json with serde_json (persistence.rs), then read back."
    expect_ok "POST /save writes state.json" POST /save
    echo "  ${DIM}positions in state.json:${RESET}"
    jq -c '.positions | to_entries[] | {(.key): .value}' state.json | sed 's/^/    /'
    expect_ok "POST /load reads it back" POST /load
}

finish() {
    echo
    echo "${BOLD}Demo finished.${RESET} The server stops now. To start it yourself: cargo run --release"
}

check_requirements
banner
start_server
pause
open_loans
pause
show_errors
pause
replay_crash
pause
run_stress_test
pause
save_state
finish
