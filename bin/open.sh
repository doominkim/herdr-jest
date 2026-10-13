#!/bin/sh
dir=$(printf '%s' "$HERDR_PLUGIN_CONTEXT_JSON" | /usr/bin/jq -r '.focused_pane_cwd // .workspace_cwd // empty')
[ -n "$dir" ] || dir=$PWD
exec "$HERDR_PLUGIN_ROOT/target/release/herdr-jest" ui --cwd "$dir"
