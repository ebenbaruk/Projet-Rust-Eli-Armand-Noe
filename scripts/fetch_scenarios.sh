#!/usr/bin/env bash
# Downloads the ETH price of the three historical crashes from the public
# Binance API (1-minute candles, ETH/USDT) and writes them to data/.
#
# Run it once from the root of the project: ./scripts/fetch_scenarios.sh
# The project only reads the JSON files afterwards, so it never needs the network.
# Requires curl and jq.

set -euo pipefail

API_URL="https://api.binance.com/api/v3/klines"
SYMBOL="ETHUSDT"
# Binance returns at most 1000 candles per request.
CANDLES_PER_REQUEST=1000
MINUTE_MS=60000
DATA_DIR="data"

# Converts an ISO 8601 date ("2020-03-12T00:00:00Z") to milliseconds since 1970.
to_ms() {
    jq --null-input --arg date "$1" '$date | fromdateiso8601 * 1000'
}

# Prints the close prices between two dates, one per line, in USDC cents.
fetch_prices() {
    local start_ms=$1
    local end_ms=$2
    while ((start_ms < end_ms)); do
        curl --silent --fail \
            "$API_URL?symbol=$SYMBOL&interval=1m&startTime=$start_ms&endTime=$((end_ms - 1))&limit=$CANDLES_PER_REQUEST" |
            # Index 4 of a candle is the close price, as a string in USDT ("194.12000000").
            jq '.[] | .[4] | tonumber * 100 | round'
        start_ms=$((start_ms + CANDLES_PER_REQUEST * MINUTE_MS))
    done
}

# Writes data/<id>.json with the format of section 5.2 of the specification.
write_scenario() {
    local id=$1
    local name=$2
    local start=$3
    local end=$4
    local start_ms end_ms expected count
    start_ms=$(to_ms "$start")
    end_ms=$(to_ms "$end")
    expected=$(((end_ms - start_ms) / MINUTE_MS))

    fetch_prices "$start_ms" "$end_ms" |
        jq --slurp --compact-output --arg id "$id" --arg name "$name" --arg start "$start" \
            '{id: $id, name: $name, start: $start, prices: .}' \
            >"$DATA_DIR/$id.json"

    count=$(jq '.prices | length' "$DATA_DIR/$id.json")
    if ((count != expected)); then
        echo "$id: $count prices instead of $expected, some minutes are missing" >&2
        exit 1
    fi
    echo "$id: $count prices"
}

mkdir -p "$DATA_DIR"
write_scenario "black-thursday" "Black Thursday" "2020-03-12T00:00:00Z" "2020-03-13T00:00:00Z"
write_scenario "luna" "LUNA" "2022-05-09T00:00:00Z" "2022-05-13T00:00:00Z"
write_scenario "ftx" "FTX" "2022-11-07T00:00:00Z" "2022-11-11T00:00:00Z"
