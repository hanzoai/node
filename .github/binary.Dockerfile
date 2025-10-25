FROM ubuntu:24.10 AS downloader
 RUN apt-get update && apt-get install -y curl unzip
 ARG HANZO_NODE_VERSION
 RUN curl -L -o hanzo-node.zip https://download.hanzo.com/hanzo-node/binaries/production/x86_64-unknown-linux-gnu/${HANZO_NODE_VERSION:-latest}.zip
 RUN FILE_SIZE=$(stat -c %s /hanzo-node.zip) && \
    if [ $FILE_SIZE -lt 26214400 ]; then \
        echo "Error: hanzo-node file is less than 25MB" && \
        exit 1; \
    fi
 RUN unzip -o hanzo-node.zip -d ./node
 RUN chmod +x /node/hanzo-node

 FROM ubuntu:24.10 AS runner
 RUN apt-get update && apt-get install -y openssl ca-certificates
 WORKDIR /app
 COPY --from=downloader /node ./

 ENV HANZO_TOOLS_RUNNER_DENO_BINARY_PATH="/app/hanzo-tools-runner-resources/deno"
 ENV HANZO_TOOLS_RUNNER_UV_BINARY_PATH="/app/hanzo-tools-runner-resources/uv"
 ENV PATH="/app/hanzo-tools-runner-resources:/root/.local/bin:$PATH"

 EXPOSE 9550
 ENTRYPOINT ["/bin/sh", "-c", "/app/hanzo-node"]