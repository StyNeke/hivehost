#!/usr/bin/env python3
"""Send one request to the host agent and print the JSON reply.

usage: agent-call.py SOCKET OPERATION_JSON
example: agent-call.py /run/hivehost/agent.sock '{"type":"inspect_host"}'
Must run as the user named by GM_API_UID; the agent rejects other peers.
"""
import json
import socket
import struct
import sys
import uuid

path, operation = sys.argv[1], json.loads(sys.argv[2])
request = json.dumps({"version": 1, "request_id": str(uuid.uuid4()), "operation": operation}).encode()
with socket.socket(socket.AF_UNIX) as connection:
    connection.connect(path)
    connection.sendall(struct.pack(">I", len(request)) + request)
    length = struct.unpack(">I", connection.recv(4, socket.MSG_WAITALL))[0]
    print(json.dumps(json.loads(connection.recv(length, socket.MSG_WAITALL))["result"], indent=1))
