#!/usr/bin/env bash
# scripts/ci/commit-hygiene.sh — no commit may carry assistant attribution or an
# agent/hostname identity.
#
# Usage:
#   commit-hygiene.sh <rev-range>...        check every commit selected by `git rev-list`
#                                           (honours GIT_DIR and the current directory)
#   commit-hygiene.sh --message-file <path> check one commit message (the commit-msg hook);
#                                           `#` comment lines and everything below a
#                                           scissors line are ignored
#   commit-hygiene.sh --self-test           run the built-in fixtures
#
# A commit fails when a message line matches (case-insensitively) one of the
# MESSAGE_RULES below, or when its author or committer e-mail matches one of the
# IDENTITY_RULES. One line is printed per violation (short SHA, rule, offending
# text) followed by a summary line; the exit status is 1 on any violation.
#
# Portable to macOS bash 3.2 and Linux: no associative arrays, no mapfile.

set -euo pipefail

# "name|extended regex" — matched per line, case-insensitively.
MESSAGE_RULES=(
  'session-trailer|^[[:space:]]*Claude-Session:'
  'session-link|claude\.ai/(code|chat|share)/'
  'co-author|^[[:space:]]*Co-Authored-By:.*(Claude|noreply@anthropic\.com)'
  'generated-with|Generated with \[?Claude'
)

# "name|extended regex" — matched against the author and committer e-mail.
IDENTITY_RULES=(
  'hostname-email|\.(local|lan)$'
  'agent-email|^claude_delta@'
  'conductor-email|^conductor@'
)

SCISSORS='# ------------------------ >8 ------------------------'

# Matching uses bash's own regex operator (no forks): the whole range of a large
# history is checked in one `git log` pass.
shopt -s nocasematch

# check_message_line <label> <line> : prints one line per violated rule; returns 1 on any.
check_message_line() {
  local label="$1" line="$2" found=0 rule name re
  for rule in "${MESSAGE_RULES[@]}"; do
    name="${rule%%|*}"
    re="${rule#*|}"
    if [[ "$line" =~ $re ]]; then
      printf '%s message %s: %s\n' "$label" "$name" "$line"
      found=1
    fi
  done
  [ "$found" -eq 0 ]
}

# check_message_text <label> : message on stdin; returns 0 when clean, 1 otherwise.
check_message_text() {
  local label="$1" found=0 line
  while IFS= read -r line || [ -n "$line" ]; do
    check_message_line "$label" "$line" || found=1
  done
  [ "$found" -eq 0 ]
}

# check_identity <label> <role> <email> : prints violations, returns 1 on any.
check_identity() {
  local label="$1" role="$2" email="$3" found=0 rule name re
  for rule in "${IDENTITY_RULES[@]}"; do
    name="${rule%%|*}"
    re="${rule#*|}"
    if [[ "$email" =~ $re ]]; then
      printf '%s identity %s (%s): %s\n' "$label" "$name" "$role" "$email"
      found=1
    fi
  done
  [ "$found" -eq 0 ]
}

# strip_message_file <path> : drop comment lines and everything below the scissors line.
strip_message_file() {
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    [ "$line" = "$SCISSORS" ] && break
    case "$line" in '#'*) continue ;; esac
    printf '%s\n' "$line"
  done <"$1"
}

check_range() {
  local msg_bad=0 id_bad=0 total=0 line label="" ae ce rest
  local cur_msg_bad=0 cur_id_bad=0
  # Records start with \001; fields are separated by \002; the body follows.
  finish() {
    if [ -n "$label" ]; then
      total=$((total + 1))
      msg_bad=$((msg_bad + cur_msg_bad))
      id_bad=$((id_bad + cur_id_bad))
    fi
    cur_msg_bad=0
    cur_id_bad=0
  }
  while IFS= read -r line || [ -n "$line" ]; do
    if [[ "$line" == $'\001'* ]]; then
      finish
      rest="${line#$'\001'}"
      label="${rest%%$'\002'*}"; rest="${rest#*$'\002'}"
      ae="${rest%%$'\002'*}"; rest="${rest#*$'\002'}"
      ce="${rest%%$'\002'*}"; line="${rest#*$'\002'}"
      check_identity "$label" author "$ae" || cur_id_bad=1
      check_identity "$label" committer "$ce" || cur_id_bad=1
    fi
    check_message_line "$label" "$line" || cur_msg_bad=1
  done < <(git log --abbrev=8 --format=$'%x01%h%x02%ae%x02%ce%x02%B' "$@")
  finish
  printf '%d commit(s) with message violations, %d commit(s) with identity violations\n' \
    "$msg_bad" "$id_bad"
  [ $((msg_bad + id_bad)) -eq 0 ]
}

self_test() {
  local failures=0 out
  expect_dirty() { # <description> <message>
    if out="$(printf '%s\n' "$2" | check_message_text fixture)"; then
      echo "self-test FAIL: not flagged: $1"; failures=$((failures + 1))
    else
      echo "self-test ok:   flagged: $1"
    fi
  }
  expect_clean() { # <description> <message>
    if out="$(printf '%s\n' "$2" | check_message_text fixture)"; then
      echo "self-test ok:   clean: $1"
    else
      echo "self-test FAIL: wrongly flagged: $1 ($out)"; failures=$((failures + 1))
    fi
  }
  expect_id_dirty() { # <email>
    if check_identity fixture author "$1" >/dev/null; then
      echo "self-test FAIL: identity not flagged: $1"; failures=$((failures + 1))
    else
      echo "self-test ok:   identity flagged: $1"
    fi
  }
  expect_id_clean() { # <email>
    if check_identity fixture author "$1" >/dev/null; then
      echo "self-test ok:   identity clean: $1"
    else
      echo "self-test FAIL: identity wrongly flagged: $1"; failures=$((failures + 1))
    fi
  }

  expect_dirty "session trailer" $'Subject\n\nclaude-session: https://example.invalid/x'
  expect_dirty "session link" $'Subject\n\nSee https://claude.ai/code/session_abc'
  expect_dirty "co-author trailer (name)" $'Subject\n\nCo-authored-by: Claude <someone@example.invalid>'
  expect_dirty "co-author trailer (address)" $'Subject\n\nCo-Authored-By: Bot <noreply@anthropic.com>'
  expect_dirty "generated-with line" $'Subject\n\nGenerated with [Claude Code]'
  expect_clean "plain message" $'Add a feature\n\nCo-authored-by: A Human <human@example.invalid>'
  expect_id_dirty "box.local"
  expect_id_dirty "dev@build.lan"
  expect_id_dirty "claude_delta@example.invalid"
  expect_id_dirty "conductor@example.invalid"
  expect_id_clean "person@example.com"
  expect_id_clean "localdev@example.com"

  if [ "$failures" -ne 0 ]; then
    echo "self-test: $failures failure(s)"
    return 1
  fi
  echo "self-test: all fixtures behaved"
}

main() {
  if [ "$#" -eq 0 ]; then
    echo "usage: $0 <rev-range>... | --message-file <path> | --self-test" >&2
    exit 2
  fi
  case "$1" in
    --self-test)
      self_test
      ;;
    --message-file)
      [ "$#" -eq 2 ] || { echo "usage: $0 --message-file <path>" >&2; exit 2; }
      if strip_message_file "$2" | check_message_text "commit-msg"; then
        exit 0
      fi
      echo "commit message rejected: remove the attribution lines above" >&2
      exit 1
      ;;
    *)
      check_range "$@"
      ;;
  esac
}

main "$@"
