# Store And Fforward server: staff

A store and forward server, used to take messages via HTTP/protobuf
and present them for later use by a pull server.


A single message is considered a `blob` - it is binary data sent to a
`:topic` in a protobuf format.

To retrieve a single message, a GET is sent to the `:topic/blob`.

To send a single message, a POST is sent to the `:topic/blob`.

To send a many messages, a POST is sent to the `:topic/blob`.

To retrieve "all" messages, a GET is sent to the `:topic/blobs`.

To check the current topic size, a HEAD is sent to the `:topic/blobs`.

To clear the topic, a DELETE is sent to the `:topic/blobs`.

The data formats are specified by protos/staff.proto.


## Advanced

A push functionality is present, but inadequately tested, where,
theoretically, a daemon thread fowards all of the messages in all
topics to another `staff` daemon and then clears the topic.

# API

## Basic API:


- POST /api/v1/:topic/blob
- HEAD /api/v1/:/topic/blobs
- GET /api/v1/:/topic/blob
- GET /api/v1/:/topic/blobs

## Advanced API

- GET /api/v1/forward
- POST /api/v1/forward
- POST /api/v1/forward/toggle

# hacking

(as of 1.0.0 this is primarily a proof of concept)

You may build in either cargo or bazel

to regenerate the proto->rust, you will need to:
1. have protoc-gen-prost installed
2. run make-protos.sh


note that tokio/rt-multi-thread is used. This was to experiment with
async Rust. the forwarding server is a daemon std::thread. Could run
into issues there idk.

Note that the code is relatively "single file of doom" style, it could
stand to be reorganized.

contributions welcome if this looks useful


# license

(C) 2024 Upside Down Research, licensed as AGPL3.
