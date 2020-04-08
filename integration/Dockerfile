FROM debian:stretch-slim

ARG BUILD_TIME

RUN [ -n "$BUILD_TIME" ] && echo "Build time $BUILD_TIME" > /build_time

ENTRYPOINT ["/bin/bash", "-c", "cat /build_time ; sleep infinity"]
