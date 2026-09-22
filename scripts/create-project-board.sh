#!/usr/bin/env bash
# Creates the "Crate Digger v1 Roadmap" GitHub Project (Projects v2) from the
# epics in docs/roadmap.md: adds issues #1-#22 in priority order and sets
# Status (Now/Next/Later/Done), Priority, Rank and Milestone fields.
#
# Requirements: gh CLI and jq, authenticated with the project scope:
#   gh auth refresh -s project
#
# Usage: scripts/create-project-board.sh [owner] [repo]
set -euo pipefail

OWNER="${1:-timini}"
REPO="${2:-crate-digger}"
TITLE="Crate Digger v1 Roadmap"

# rank issue priority status milestone  (keep in sync with docs/roadmap.md)
EPICS=(
  "1 1 P0 Now M1"
  "2 2 P0 Now M1"
  "3 3 P0 Next M1"
  "4 4 P0 Next M1"
  "5 5 P0 Next M1"
  "6 6 P0 Next M1"
  "7 7 P1 Next M1"
  "8 8 P1 Next M2"
  "9 9 P1 Later M2"
  "10 10 P1 Later M3"
  "11 11 P1 Later M3"
  "12 12 P1 Later M3"
  "13 13 P1 Later M3"
  "14 14 P1 Later M3"
  "15 15 P1 Later M3"
  "16 16 P2 Later M3"
  "17 20 P2 Later M5"
  "18 17 P2 Later M4"
  "19 18 P2 Later M4"
  "20 21 P2 Later M5"
  "21 19 P3 Later M4"
  "22 22 P3 Later M5"
)

declare -A MILESTONE_NAMES=(
  [M1]="M1 Local foundation"
  [M2]="M2 Analysis & identity"
  [M3]="M3 End-to-end discovery"
  [M4]="M4 Central service"
  [M5]="M5 Release"
)

command -v gh >/dev/null || { echo "gh CLI is required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

echo "Creating project \"$TITLE\" for $OWNER..."
project_json=$(gh project create --owner "$OWNER" --title "$TITLE" --format json)
PROJECT_NUMBER=$(jq -r '.number' <<<"$project_json")
PROJECT_ID=$(jq -r '.id' <<<"$project_json")
PROJECT_URL=$(jq -r '.url' <<<"$project_json")

gh project link "$PROJECT_NUMBER" --owner "$OWNER" --repo "$OWNER/$REPO" >/dev/null
gh project edit "$PROJECT_NUMBER" --owner "$OWNER" \
  --description "v1 epics from the product spec, ranked. See docs/roadmap.md." >/dev/null

# Replace the default Status options (Todo/In Progress/Done) with the board columns.
status_field_id=$(gh project field-list "$PROJECT_NUMBER" --owner "$OWNER" --format json \
  | jq -r '.fields[] | select(.name == "Status") | .id')
STATUS_FIELD="Status"
if ! gh api graphql -f field="$status_field_id" -f query='
  mutation($field: ID!) {
    updateProjectV2Field(input: {
      fieldId: $field
      singleSelectOptions: [
        {name: "Now",   color: ORANGE, description: "In progress (keep to 2-3 epics)"}
        {name: "Next",  color: YELLOW, description: "Ready to start when a Now slot frees up"}
        {name: "Later", color: GRAY,   description: "Prioritised backlog"}
        {name: "Done",  color: GREEN,  description: "Completed"}
      ]
    }) { projectV2Field { ... on ProjectV2SingleSelectField { id } } }
  }' >/dev/null 2>&1; then
  echo "Could not edit the built-in Status field; creating a \"Stage\" field instead."
  STATUS_FIELD="Stage"
  gh project field-create "$PROJECT_NUMBER" --owner "$OWNER" --name Stage \
    --data-type SINGLE_SELECT --single-select-options "Now,Next,Later,Done" >/dev/null
fi

gh project field-create "$PROJECT_NUMBER" --owner "$OWNER" --name Priority \
  --data-type SINGLE_SELECT --single-select-options "P0,P1,P2,P3" >/dev/null
gh project field-create "$PROJECT_NUMBER" --owner "$OWNER" --name Rank \
  --data-type NUMBER >/dev/null
milestone_options=$(printf '%s,' "${MILESTONE_NAMES[M1]}" "${MILESTONE_NAMES[M2]}" \
  "${MILESTONE_NAMES[M3]}" "${MILESTONE_NAMES[M4]}" "${MILESTONE_NAMES[M5]}")
gh project field-create "$PROJECT_NUMBER" --owner "$OWNER" --name "Delivery milestone" \
  --data-type SINGLE_SELECT --single-select-options "${milestone_options%,}" >/dev/null

fields=$(gh project field-list "$PROJECT_NUMBER" --owner "$OWNER" --format json)
field_id() { jq -r --arg n "$1" '.fields[] | select(.name == $n) | .id' <<<"$fields"; }
option_id() {
  jq -r --arg n "$1" --arg o "$2" \
    '.fields[] | select(.name == $n) | .options[] | select(.name == $o) | .id' <<<"$fields"
}
STATUS_ID=$(field_id "$STATUS_FIELD")
PRIORITY_ID=$(field_id Priority)
RANK_ID=$(field_id Rank)
MILESTONE_ID=$(field_id "Delivery milestone")

set_select() {
  gh project item-edit --project-id "$PROJECT_ID" --id "$1" \
    --field-id "$2" --single-select-option-id "$3" >/dev/null
}

for row in "${EPICS[@]}"; do
  read -r rank issue priority status milestone <<<"$row"
  echo "  #$issue  rank $rank  $priority  $status"
  item_id=$(gh project item-add "$PROJECT_NUMBER" --owner "$OWNER" \
    --url "https://github.com/$OWNER/$REPO/issues/$issue" --format json | jq -r '.id')
  set_select "$item_id" "$STATUS_ID" "$(option_id "$STATUS_FIELD" "$status")"
  set_select "$item_id" "$PRIORITY_ID" "$(option_id Priority "$priority")"
  set_select "$item_id" "$MILESTONE_ID" "$(option_id "Delivery milestone" "${MILESTONE_NAMES[$milestone]}")"
  gh project item-edit --project-id "$PROJECT_ID" --id "$item_id" \
    --field-id "$RANK_ID" --number "$rank" >/dev/null
done

cat <<EOF

Done: $PROJECT_URL

The GitHub API can't create views, so finish in the browser (about 30 seconds):
  1. Open the project, then View 1 > Layout > Board.
  2. Set "Column by" to "$STATUS_FIELD".
  3. Sort by "Rank" (ascending), then Save.
EOF
