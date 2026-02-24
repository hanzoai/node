#!/bin/bash

# export INSTALL_FOLDER_PATH=${INSTALL_FOLDER_PATH:-"/app/pre-install"}
cd /app && cargo test -p hanzo-messages -- --test-threads=1 --nocapture

