#!/bin/bash

# This script derives the next tag from the current branch and the previously generated tags
# A new tag is only created if a new commit has been detected compared to the previous tag.
# The tag has the following format V{MAJOR}.${MINOR}{suffix}{increment}
# If it executes on a develop branch : {suffix} is set to DB  (e.g first tag for V26: V26.0DB1)
# If it executes on a releases/x{Major} branch : {suffix} is set to RC (e.g first tag for V26: V26.0RC1)
# If it executes on any other branch, the {suffix} is set to the branch_name

# if -c flag is provided, version_pre_release in CMakeLists.txt is incremented and a new tag is created and pushed to origin
# if -o is provided, "build_tag" , "version_pre_release" and "tag_created" are written to file
# if -r flag is set (release_build=true), we ignore the {suffix} 
#   --> if there is no new commit, the same tag is generated again (or V{MAJOR}.{MINOR} if no tag exists yet)
#   --> if there is a new commit, we increment the {MINOR} version


#!/bin/bash

set -e
set -x 
output=""
create=false
tag_created="false"
release_build=false
branch_name=""

while getopts ":o:c:rb:" opt; do
  case ${opt} in
    o )
      output=$OPTARG
      ;;
    c )
      create=true
      ;;
    r )
      release_build=true
      ;;
    b )
      branch_name=$OPTARG
      ;;
    \? )
      echo "Invalid Option: -$OPTARG" 1>&2
      exit 1
      ;;
    : )
      echo "Invalid Option: -$OPTARG requires an argument" 1>&2
      exit 1
      ;;
  esac
done
shift $((OPTIND -1))


get_tag_suffix() {
    local branch_name=$1
    local tag_suffix=${branch_name//[^a-zA-Z0-9]/_}

    if [[ "$branch_name" == "develop" ]]; then
        tag_suffix="DB"
    elif [[ "$branch_name" =~ ^releases/v[0-9]+ ]]; then
        tag_suffix="RC"
    fi

    echo $tag_suffix
}


update_output_file() {   
    local new_tag=$1
    local next_number=$2
    local tag_created=$3
    local tag_type=$4

    if [[ -n "$output" ]]; then
        echo "build_tag =$new_tag" > $output
        echo "$tag_type =$next_number" >> $output
        echo "tag_created =$tag_created" >> $output
    fi
}

update_cmake_lists() {
    local tag_type=$1
    local next_number=$2
    local variable_to_update=""

    if [[ "$tag_type" == "version_pre_release" ]]; then
        variable_to_update="CPACK_PACKAGE_VERSION_PRE_RELEASE"
    elif [[ "$tag_type" == "version_minor" ]]; then
        variable_to_update="CPACK_PACKAGE_VERSION_MINOR"
    fi

    if [[ -n "$variable_to_update" ]]; then
        echo "Update ${variable_to_update} to $next_number"
        sed -i.bak "s/set(${variable_to_update} \"[0-9]*\")/set(${variable_to_update} \"${next_number}\")/g" CMakeLists.txt
        rm CMakeLists.txt.bak        
    fi
    git add CMakeLists.txt
}

function create_commit() {
    git diff --cached --quiet
    local has_changes=$? # store exit status of the last command
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

# Fetch the last commit hash of the current branch
current_commit_hash=$(git rev-parse HEAD)

# If no branch name is provided through the -b option, fetch the current branch name
if [[ -z "$branch_name" ]]; then
    branch_name=$(git rev-parse --abbrev-ref HEAD)
fi

# Fetch major and minor version numbers from CMakeLists.txt
current_version_major=$(grep "CPACK_PACKAGE_VERSION_MAJOR" CMakeLists.txt | grep -o "[0-9]\+")
current_version_minor=$(grep "CPACK_PACKAGE_VERSION_MINOR" CMakeLists.txt | grep -o "[0-9]\+")


# Check if it's a release branch
is_release_branch=$(echo "$branch_name" | grep -q "releases/v$current_version_major" && echo true || echo false)

# Determine the tag type and base version format
if [[ $is_release_branch == true && $release_build == true ]]; then
    tag_type="version_minor"
    base_version="V${current_version_major}.${current_version_minor}"
else
    tag_type="version_pre_release"
    tag_suffix=$(get_tag_suffix "$branch_name")
    base_version="V${current_version_major}.${current_version_minor}${tag_suffix}"
fi

# Fetch existing tags based on the base version
existing_tags=$(git tag --list "${base_version}*" | grep -E "${base_version}[0-9]*$" || true)

# Determine the initial tag number based on branch type and release build status
if [[ $is_release_branch == true && $release_build == true ]]; then
    next_number=0  # Start from 0 for release branches with release build
else
    next_number=1  # Start from 1 for other branches
fi

if [[ -z "$existing_tags" ]]; then
    tag_created="true"
else
    last_tag=$(echo "$existing_tags" | sort -V | tail -n1)
    last_tag_commit_hash=$(git rev-list -n 1 "$last_tag")

    if [[ "$current_commit_hash" == "$last_tag_commit_hash" ]]; then
        echo "No new commits since the last tag. No new tag will be created."
        tag_created="false"
    else
        tag_created="true"
        if [[ $is_release_branch == true && $release_build == true ]]; then
            # Increment the minor version for release branches when -r flag is set
            next_number=$((current_version_minor + 1))
        else
            # Increment the suffix number for non-release branches
            last_tag_number=$(echo "$last_tag" | awk -F"${tag_suffix}" '{print $2}')
            next_number=$((last_tag_number + 1))
        fi
    fi
fi

# Generate the new tag
if [[ $is_release_branch == true && $release_build == true ]]; then
    new_tag="V${current_version_major}.${next_number}"
else
    new_tag="${base_version}${next_number}"
fi

update_output_file $new_tag $next_number $tag_created $tag_type

# Skip tag creation if no new commits
if [[ "$tag_created" == "true" ]]; then
    echo "$new_tag"
else
    exit 0
fi

if [[ $create == true ]]; then
    # Stash current changes
    git config user.name "${GITHUB_ACTOR}"
    git config user.email "${GITHUB_ACTOR}@users.noreply.github.com"

    # Update variable in CMakeLists.txt
    update_cmake_lists "$tag_type" "$next_number"

    commit_made=$(create_commit)

    git tag -fa "$new_tag" -m "This tag was created with generate_next_git_tag.sh"
    git push origin "$new_tag" -f
    echo "The tag $new_tag has been created and pushed."

    # If it's a release branch, also push the commit to the branch
    if [[ $is_release_branch == true ]]; then
        git push origin "$branch_name" -f
        echo "The commit has been pushed to the $branch_name branch."
    fi

    # Only reset local branch if a commit was made and it's not a "releases" branch.
    if [[ "$commit_made" == "true" && $is_release_branch == false ]]; then
        git reset --hard HEAD~1
        echo "The commit used for the tag does not exist on any branch."
    fi
fi