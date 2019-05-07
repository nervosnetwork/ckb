#!/bin/bash
set -ev

cargo sweep -s

# Run test only in master branch and pull requests
RUN_TEST=false
# Run integration only in master, develop and rc branches
RUN_INTEGRATION=false
if [ "$TRAVIS_PULL_REQUEST" != false ]; then
  LAST_COMMIT_MSG="$(git log --max-count 1 --skip 1 --format="%s")"
  echo "Last commit message is \"${LAST_COMMIT_MSG}\""
  if [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[skip\ tests\]\  ]]; then
      :
  elif [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[only\ integration\]\  ]]; then
    RUN_INTEGRATION=true
  elif [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[all\ tests\]\  ]]; then
    RUN_TEST=true
    RUN_INTEGRATION=true
  else
    RUN_TEST=true
  fi
else
  RUN_INTEGRATION=true
  if [ "$TRAVIS_BRANCH" = master ]; then
    RUN_TEST=true
  fi
fi

echo "\${RUN_TEST} = ${RUN_TEST}"
echo "\${RUN_INTEGRATION} = ${RUN_INTEGRATION}"

if [ "$RUN_TEST" = true ]; then
  if [ "$FMT" = true ]; then
    make fmt
  fi
  if [ "$CHECK" = true ]; then
    make check
    make clippy
    make security-audit
  fi
  if [ "$TEST" = true ]; then
    make test
  fi

  git diff --exit-code Cargo.lock
fi

# We'll create PR for develop and rc branches to trigger the integration test.
if [ "$RUN_INTEGRATION" = true ]; then
  echo "Running integration test..."
  make integration

  # Switch to release mode when the running time is much longer than the build time.
  # make integration-release
else
  echo "Skip integration test..."
fi

# Publish package for release
if [ -n "$TRAVIS_TAG" -a -n "$GITHUB_TOKEN" -a -n "$REL_PKG" ]; then
  git fetch --unshallow
  make prod
  rm -rf releases
  mkdir releases
  PKG_NAME="ckb_${TRAVIS_TAG}_${REL_PKG%%.*}"
  mkdir "releases/$PKG_NAME"
  mv target/release/ckb "releases/$PKG_NAME"
  cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
  cp -R devtools/init "releases/$PKG_NAME"
  cp -R docs "releases/$PKG_NAME"
  cp rpc/README.md "releases/$PKG_NAME/docs/rpc.md"

  pushd releases
  if [ "${REL_PKG#*.}" = "tar.gz" ]; then
    tar -czf $PKG_NAME.tar.gz $PKG_NAME
  else
    zip -r $PKG_NAME.zip $PKG_NAME
  fi
  popd
fi
