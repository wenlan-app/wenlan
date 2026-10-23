#!/usr/bin/env bash
# Resolve client-local Wenlan Space context.
# Precedence: strict pin > explicit arg > default env > cwd mapping >
# registered repo basename > unscoped. The daemon owns the save default.
#
# Default output: one line, "<space>\t<layer>" (space is empty when unscoped).
# --args "<raw>": skill mode. <raw> is the raw skill argument string. The first
# `space:<name>` token is stripped and used as --arg. Output becomes one line,
# "<space>\t<layer>\t<rest>" (rest = <raw> without the token, trimmed, may be
# empty), and stderr gets "Resolved space: <space> (from <layer>)" or
# "Resolved space: none (unscoped)". Exit status is always 0.

set -u

cwd=""
arg=""
new_repo_fallback=0
skill_mode=0
rest=""

while [ $# -gt 0 ]; do
    case "$1" in
        --cwd) cwd="${2:-}"; shift $(( $# > 1 ? 2 : 1 )) ;;
        --arg) arg="${2:-}"; shift $(( $# > 1 ? 2 : 1 )) ;;
        --args)
            skill_mode=1
            raw="${2:-}"
            shift $(( $# > 1 ? 2 : 1 ))
            token="$(printf '%s\n' "$raw" | grep -oE 'space:[A-Za-z0-9_-]+' | head -1 | cut -d: -f2)"
            [ -n "$token" ] && arg="$token"
            rest="$(printf '%s\n' "$raw" | tr '\n' ' ' | sed -E 's/[[:space:]]*space:[A-Za-z0-9_-]+[[:space:]]*/ /g' | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')"
            ;;
        --topic) shift $(( $# > 1 ? 2 : 1 )) ;;
        --new-repo-fallback) new_repo_fallback=1; shift ;;
        *) shift ;;
    esac
done

trim() {
    printf '%s' "$1" | awk '{
        sub(/^[[:space:]]+/, "")
        sub(/[[:space:]]+$/, "")
        print
    }'
}

# emit <space> <layer>: print the result line, then exit 0.
emit() {
    if [ "$skill_mode" = "1" ]; then
        printf '%s\t%s\t%s\n' "$1" "$2" "$rest"
        if [ -n "$1" ]; then
            printf 'Resolved space: %s (from %s)\n' "$1" "$2" >&2
        else
            printf 'Resolved space: none (unscoped)\n' >&2
        fi
    else
        printf '%s\t%s\n' "$1" "$2"
    fi
    exit 0
}

locked="$(trim "${WENLAN_SPACE:-}")"
if [ -n "$locked" ]; then
    emit "$locked" locked-env
fi

explicit="$(trim "$arg")"
if [ -n "$explicit" ]; then
    emit "$explicit" arg
fi

fallback="$(trim "${WENLAN_DEFAULT_SPACE:-}")"
if [ -n "$fallback" ]; then
    emit "$fallback" default-env
fi

spaces_file="${SPACES_FILE:-${HOME}/.wenlan/spaces.toml}"
if [ -n "$cwd" ] && [ -f "$spaces_file" ]; then
    pairs="$(mktemp)"
    awk '
        BEGIN { in_mapping = 0; prefix = ""; space = "" }
        /^\[\[mapping\]\]/ {
            if (in_mapping && prefix != "" && space != "") print prefix "\t" space
            in_mapping = 1; prefix = ""; space = ""; next
        }
        in_mapping && /^prefix[ \t]*=/ {
            sub(/^prefix[ \t]*=[ \t]*/, ""); sub(/[ \t]*$/, ""); gsub(/"/, "")
            prefix = $0
        }
        in_mapping && /^space[ \t]*=/ {
            sub(/^space[ \t]*=[ \t]*/, ""); sub(/[ \t]*$/, ""); gsub(/"/, "")
            space = $0
        }
        END {
            if (in_mapping && prefix != "" && space != "") print prefix "\t" space
        }
    ' "$spaces_file" 2>/dev/null > "$pairs"

    best_prefix=""
    best_space=""
    while IFS=$'\t' read -r prefix space; do
        case "$prefix" in
            "~/"*) expanded="${HOME}/${prefix#~/}" ;;
            "~") expanded="$HOME" ;;
            *) expanded="$prefix" ;;
        esac
        case "$cwd" in
            "$expanded"|"$expanded/"*)
                if [ ${#expanded} -gt ${#best_prefix} ]; then
                    best_prefix="$expanded"
                    best_space="$space"
                fi
                ;;
        esac
    done < "$pairs"
    rm -f "$pairs"

    if [ -n "$best_space" ]; then
        emit "$best_space" cwd-config
    fi
fi

if [ -n "$cwd" ] && [ -d "$cwd" ]; then
    repo_root="$(cd "$cwd" && git rev-parse --show-toplevel 2>/dev/null || true)"
    if [ -n "$repo_root" ]; then
        common="$(cd "$cwd" && git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
        case "$common" in
            */.git) repo_name="$(basename "$(dirname "$common")")" ;;
            *) repo_name="$(basename "$repo_root")" ;;
        esac

        wenlan_bin="${WENLAN_BIN:-wenlan}"
        if "$wenlan_bin" spaces show "$repo_name" --format json >/dev/null 2>&1; then
            emit "$repo_name" cwd-repo
        elif [ "$new_repo_fallback" = "1" ]; then
            emit "$repo_name" cwd-repo-new
        fi
    fi
fi

emit "" unscoped
