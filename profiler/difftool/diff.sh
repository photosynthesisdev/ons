#!/bin/bash

# Paths to the FlameGraph repository and input folders
FLAMEGRAPH_DIR="../../FlameGraph"
DTLS_FOLDED_FILE="../dtls_graph/out.folded"
WEBTRANSPORT_FOLDED_FILE="../webtransport_graph/out.folded"
OUTPUT_DIFF="diff_flamegraph.svg"

# Check if the necessary folded files exist
if [ ! -f "$DTLS_FOLDED_FILE" ] || [ ! -f "$WEBTRANSPORT_FOLDED_FILE" ]; then
  echo "Error: Both $DTLS_FOLDED_FILE and $WEBTRANSPORT_FOLDED_FILE must exist."
  exit 1
fi

# Generate differential folded output
echo "Creating differential folded stack trace..."
"$FLAMEGRAPH_DIR/difffolded.pl" "$WEBTRANSPORT_FOLDED_FILE" "$DTLS_FOLDED_FILE" > diff.folded

# Check if diff.folded was created successfully
if [ ! -f "diff.folded" ]; then
  echo "Error: Failed to create diff.folded."
  exit 1
fi

# Generate differential flame graph from diff.folded
echo "Generating differential flame graph..."
"$FLAMEGRAPH_DIR/flamegraph.pl" diff.folded > "$OUTPUT_DIFF"

# Final output message
if [ -f "$OUTPUT_DIFF" ]; then
  echo "Differential flame graph created: $OUTPUT_DIFF"
else
  echo "Error: Failed to generate differential flame graph."
fi
