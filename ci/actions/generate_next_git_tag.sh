#!/bin/bash

# Script Description

# Purpose:
# This script generates a new Git tag based on the current branch and previously generated tags.
# It creates a new tag only if there's a new commit compared to the previous tag.

# Tag Format:
# General: V{MAJOR}.{MINOR}{tag_suffix}{increment}
# For releases/ branch: V{MAJOR}.{MINOR}

# Options:
# -r : Indicates a release build. In this case, {tag_suffix} is ignored.
#   New commit: Increments the {MINOR} version.
#   No new commit: Generates the same tag again or V{MAJOR}.{MINOR} if no tag exists.
# -s {tag_suffix} : Use a custom {tag_suffix} instead of the default derived from the branch name.
#   DB for develop branch (e.g., V26.0DB1)
#   RC for releases/x{Major} branch (e.g. V26.0RC1)
#   {branch_name} for other branches (e.g. V26.0prs_refactor_tag_generation1)
# -c : Create and push the tag to origin.
# -o {output} : Write results to the specified output file.

set -e
set -x
output=""
push_tag=false
tag_created="false"
is_release_build=${IS_RELEASE_BUILD:-false} 
tag_suffix=""

while getopts ":ro:cs:" opt; do
    case ${opt} in
    r)
        is_release_build=true
        ;;
    o)
        output=$OPTARG
        ;;
    c)
        push_tag=true
        ;;
    s)
        tag_suffix=$OPTARG
        ;;
    \?)
        echo "Invalid Option: -$OPTARG" 1>&2
        exit 1
        ;;
    :)
        echo "Invalid Option: -$OPTARG requires an argument" 1>&2
        exit 1
        ;;
    esac
done
shift $((OPTIND - 1))

get_tag_suffix() {
    local existing_suffix=$1
    local branch_name=$2

    # If tag_suffix is already provided, return it
    if [[ -n "$existing_suffix" ]]; then
        echo "$existing_suffix"
        return
    fi

    # Replace non-alphanumeric characters with underscores
    local new_tag_suffix=${branch_name//[^a-zA-Z0-9]/_}

    # Specific rules for certain branch names
    if [[ "$branch_name" == "develop" ]]; then
        new_tag_suffix="DB"
    elif [[ "$branch_name" =~ ^releases/v[0-9]+ ]]; then
        new_tag_suffix="RC"
    fi
    echo $new_tag_suffix
}

update_output_file() {
    local new_tag=$1
    local new_tag_number=$2
    local tag_created=$3
    local tag_type=$4

    if [[ -n "$output" ]]; then
        echo "build_tag =$new_tag" >$output
        echo "$tag_type =$new_tag_number" >>$output
        echo "tag_created =$tag_created" >>$output
    fi
}

update_cmake_lists() {
    local tag_type=$1
    local new_tag_number=$2
    local variable_to_update=""

    if [[ "$tag_type" == "version_pre_release" ]]; then
        variable_to_update="CPACK_PACKAGE_VERSION_PRE_RELEASE"
    elif [[ "$tag_type" == "version_minor" ]]; then
        variable_to_update="CPACK_PACKAGE_VERSION_MINOR"
    fi

    if [[ -n "$variable_to_update" ]]; then
        echo "Update ${variable_to_update} to $new_tag_number"
        sed -i.bak "s/set(${variable_to_update} \"[0-9]*\")/set(${variable_to_update} \"${new_tag_number}\")/g" CMakeLists.txt
        rm CMakeLists.txt.bak
    fi
    git add CMakeLists.txt
}

function create_commit() {
    git diff --cached --quiet
    local has_changes=$?              # store exit status of the last command
    if [[ $has_changes -eq 0 ]]; then # no changes
        echo "No changes to commit"
        echo "false"
    else # changes detected
        git commit -m "Update CMakeLists.txt"
        echo "true"
    fi
}

# Fetch all existing tags
git fetch --tags -f

current_branch_name=$(git rev-parse --abbrev-ref HEAD)
current_commit_hash=$(git rev-parse HEAD)
current_version_major=$(grep "CPACK_PACKAGE_VERSION_MAJOR" CMakeLists.txt | grep -o "[0-9]\+")
current_version_minor=$(grep "CPACK_PACKAGE_VERSION_MINOR" CMakeLists.txt | grep -o "[0-9]\+")

# Check if it's a release branch
is_release_branch=$(echo "$current_branch_name" | grep -q "releases/v$current_version_major" && echo true || echo false)

is_release_branch_and_build() {
    [[ $is_release_branch == true && $is_release_build == true ]]
}

# Determine the tag type and base version format
if is_release_branch_and_build; then
    tag_type="version_minor"
    tag_base="V${current_version_major}.${current_version_minor}"
    tag_next_minor_number=${current_version_minor} # Use CPACK_PACKAGE_VERSION_MINOR as default
else
    tag_type="version_pre_release"
    tag_suffix=$(get_tag_suffix "$tag_suffix" "$current_branch_name")
    tag_base="V${current_version_major}.${current_version_minor}${tag_suffix}"
    tag_next_suffix_number=1 # Will be overwritten if a previous tag exists
fi

# Fetch existing tags based on the base version
existing_tags=$(git tag --list "${tag_base}*" | grep -E "${tag_base}[0-9]*$" || true)

if [[ -z "$existing_tags" ]]; then
    # create first tag if no tag exists yet
    should_create_tag="true"
else
    last_tag=$(echo "$existing_tags" | sort -V | tail -n1)
    last_tag_commit_hash=$(git rev-list -n 1 "$last_tag")

    if [[ "$current_commit_hash" == "$last_tag_commit_hash" ]]; then
        # No new commits
        should_create_tag="false"
    else
        should_create_tag="true"
        if is_release_branch_and_build; then
            # Increment the minor version for release builds (-r flag is set) based on CPACK_PACKAGE_VERSION_MINOR
            tag_next_minor_number=$((current_version_minor + 1))
        else
            # Increment the suffix number for non-release builds based on the existing tags
            last_suffix_number=$(echo "$last_tag" | awk -F"${tag_suffix}" '{print $2}')
            tag_next_suffix_number=$((last_suffix_number + 1))
        fi
    fi
fi

# Generate the new tag name
if is_release_branch_and_build; then
    new_tag="V${current_version_major}.${tag_next_minor_number}"
    new_tag_number=${tag_next_minor_number}
else
    new_tag="${tag_base}${tag_next_suffix_number}"
    new_tag_number=${tag_next_suffix_number}
fi

update_output_file $new_tag $new_tag_number $should_create_tag $tag_type

# Skip tag creation if no new commits
if [[ "$should_create_tag" == "true" ]]; then
    echo "Tag '$new_tag' ready to be created"
else
    echo "No new commits since the last tag. No new tag will be created."
    exit 0
fi

if [[ $push_tag == true ]]; then
    # Stash current changes
    git config user.name "${GITHUB_ACTOR}"
    git config user.email "${GITHUB_ACTOR}@users.noreply.github.com"

    # Update variable in CMakeLists.txt
    update_cmake_lists "$tag_type" "$new_tag_number"

    commit_made=$(create_commit)

    git tag -fa "$new_tag" -m "This tag was created with generate_next_git_tag.sh"
    git push origin "$new_tag" -f
    echo "The tag $new_tag has been created and pushed."

    # If it's a release branch, also push the commit to the branch
    if [[ $is_release_branch == true ]]; then
        git push origin "$current_branch_name" -f
        echo "The commit has been pushed to the $current_branch_name branch."

    elif [[ "$commit_made" == "true" ]]; then
        # Resets the last commit on non-release branches after tagging, keeping the current branch clean.
        git reset --hard HEAD~1
        echo "The commit used for the tag does not exist on any branch."
    fi

else
    echo "Tag was not created. Run the script with -c option to create and push the tag"
fi
