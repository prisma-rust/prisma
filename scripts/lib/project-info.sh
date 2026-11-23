#!/usr/bin/env bash
# Project info parsing (#598): shared by `new-android-game.sh` / `build-android-game.sh`.
#
# Only reads Cargo.toml and assets/settings.ini, does not guess lib name from directory name:
#   1. `[build] lib_name` in `assets/settings.ini` (explicit override, highest priority)
#   2. `[lib] name` in `Cargo.toml`
#   3. `[package] name` in `Cargo.toml` (`-` replaced with `_`, cargo default lib name rule)
#
# Usage (source from script):
#   . "$ROOT/scripts/lib/project-info.sh"
#   PROJECT_DIR=$(prism_project_dir "$ROOT" fishing)
#   LIB_NAME=$(prism_lib_name "$PROJECT_DIR")

# Read value of `[section] key` in INI (no inline comment parsing; double quotes stripped).
# If not found / empty value → output empty line and return 1.
prism_ini_get() {
    local file="$1" section="$2" key="$3"
    [ -f "$file" ] || return 1
    awk -v want_section="$section" -v want_key="$key" '
        /^[[:space:]]*[#;]/ { next }
        /^[[:space:]]*\[/ {
            line = $0
            gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", line)
            in_section = (line == want_section)
            next
        }
        in_section && /^[[:space:]]*[A-Za-z0-9_.-]+[[:space:]]*=/ {
            split($0, parts, "=")
            name = parts[1]
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
            if (name != want_key) next
            value = substr($0, index($0, "=") + 1)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
            if (value ~ /^".*"$/) value = substr(value, 2, length(value) - 2)
            print value
            found = 1
            exit
        }
        END { if (!found) exit 1 }
    ' "$file"
}

# `name = "..."` under some `[section]` in Cargo.toml.
prism_cargo_name() {
    local manifest="$1" section="$2"
    [ -f "$manifest" ] || return 1
    awk -v want="$section" '
        /^[[:space:]]*\[/ {
            line = $0
            gsub(/^[[:space:]]*\[|\][[:space:]]*$/, "", line)
            in_section = (line == want)
            next
        }
        in_section && /^[[:space:]]*name[[:space:]]*=/ {
            value = $0
            sub(/^[^=]*=[[:space:]]*"/, "", value)
            sub(/".*$/, "", value)
            print value
            found = 1
            exit
        }
        END { if (!found) exit 1 }
    ' "$manifest"
}

# `projects/<name>` (template / editor) or `projects/demos/<name>` (demo games, one
# shared workspace) or an absolute / relative path → absolute path; must have Cargo.toml.
prism_project_dir() {
    local root="$1" name="$2"
    local dir
    case "$name" in
        /*) dir="$name" ;;
        */*) dir="$root/$name" ;;
        *)
            if [ -f "$root/projects/$name/Cargo.toml" ]; then
                dir="$root/projects/$name"
            else
                dir="$root/projects/demos/$name"
            fi
            ;;
    esac
    [ -f "$dir/Cargo.toml" ] || {
                echo "Cannot find project: $dir/Cargo.toml (usage: <project> is a directory name under projects/ or projects/demos/)" >&2
        return 1
    }
    (cd "$dir" && pwd)
}

# Project lib name (.so name without lib prefix and .so suffix, also `android.app.lib_name`).
prism_lib_name() {
    local dir="$1"
    local name
    if name=$(prism_ini_get "$dir/assets/settings.ini" build lib_name) && [ -n "$name" ]; then
        printf '%s\n' "$name"
        return 0
    fi
    if name=$(prism_cargo_name "$dir/Cargo.toml" lib) && [ -n "$name" ]; then
        printf '%s\n' "$name" | tr '-' '_'
        return 0
    fi
    if name=$(prism_cargo_name "$dir/Cargo.toml" package) && [ -n "$name" ]; then
        printf '%s\n' "$name" | tr '-' '_'
        return 0
    fi
        echo "Cannot read lib name: $dir/Cargo.toml missing [package] name" >&2
    return 1
}

# Project name (directory name, used for default package name / tag).
prism_project_name() {
    basename "$1"
}
