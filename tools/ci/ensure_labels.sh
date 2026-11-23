#!/bin/sh
# Create labels used by Issue / PR automation. Idempotent; existing labels kept as-is.
#
#   sh tools/ci/ensure_labels.sh            # current repo
#   sh tools/ci/ensure_labels.sh owner/repo # specified repo
set -eu

repo=${1:-${GITHUB_REPOSITORY:-}}
repo_flag=""
[ -n "$repo" ] && repo_flag="-R $repo"

create() {
    name=$1
    color=$2
    description=$3
        # shellcheck disable=SC2086 # repo_flag needs word splitting into -R <repo>
    gh label create "$name" --color "$color" --description "$description" $repo_flag \
        >/dev/null 2>&1 || true
}

# region
create "area/render"    "1f77b4" "Vulkan rendering, shaders, swapchain"
create "area/scene"     "2ca02c" "prism-scene / reflection / RSCN"
create "area/editor"    "9467bd" "Editor and prism-editor-api"
create "area/engine"    "ff7f0e" "engine / app / platform / audio"
create "area/assets"    "8c564b" "asset / cooker / build-pipeline"
create "area/game"      "e377c2" "projects/ user game projects"
create "area/android"   "00a4a6" "Android packaging, launcher, JNI"
create "area/ci"        "7f7f7f" "CI, scripts, xtask"
create "area/docs"      "bcbd22" "Documentation"

# Size
create "size/s"         "0e8a16" "Changes < 100 lines"
create "size/m"         "fbca04" "Changes 100–499 lines"
create "size/l"         "d93f0b" "Changes >= 500 lines (should explain or split)"

# Workflow
create "ai"             "5319e7" "AI agent involved"
create "needs/title"    "b60205" "PR title not Conventional Commits"
create "needs/self-check" "b60205" "PR missing pr_ready self-check block"
create "needs/triage"   "d4c5f9" "Awaiting human / agent triage"
create "source/discord" "5865f2" "Issue transferred from Discord bot"
create "automation"     "ededed" "Item produced or handled by periodic automation"

# Agent claim
create "agent/ready"    "0e8a16" "Can be claimed by agent"
create "agent/working"  "1d76db" "Agent is working on it"
create "agent/blocked"  "b60205" "Agent blocked, needs human"
