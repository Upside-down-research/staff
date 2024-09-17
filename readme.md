# Store And Fforward server.


A store and forward server, used to take messages via HTTP and present
them for later use by a pull server.

A single message is considered a `blob` - it is binary data sent to a
`:topic`.

To retrieve a single message, a GET is sent to the `:topic/blob`.

To retrieve "all" messages, a GET is sent to the `:topic/blobs`.

To check the current size, a HEAD is sent to the `:topic/blob`.

The data formats are specified by protos/staff.proto.


API:


- POST /api/v1/:topic/blob
- HEAD /api/v1/:/topic/blob
- GET /api/v1/:/topic/blob
- GET /api/v1/:/topic/blobs
