#!/usr/bin/env python3
"""Test-environment helper: expose a host-local HTTP proxy to game containers.

Some disposable test hosts only allow outbound traffic through a CONNECT-only
proxy bound to 127.0.0.1, which containers cannot reach. This listens on
LISTEN_HOST:LISTEN_PORT (normally the managed Docker network gateway):

- CONNECT requests are tunnelled to the upstream proxy unchanged.
- Plain-HTTP proxy requests (for example apt's http:// mirrors) are re-fetched
  as HTTPS from the same host through the upstream proxy, verified with CA_FILE.
- Plain-HTTP requests to a host:port listed in GM_TEST_LOCAL_HOSTS (comma
  separated) go directly to it, for files served locally during tests.

It is not part of the product and must not be installed on the production host.

usage: egress-forwarder.py LISTEN_HOST LISTEN_PORT PROXY_HOST PROXY_PORT CA_FILE
"""
import asyncio
import os
import ssl
import sys
from urllib.parse import urlsplit


LOCAL_HOSTS = {h.strip() for h in os.environ.get("GM_TEST_LOCAL_HOSTS", "").split(",") if h.strip()}


async def pipe(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError, ssl.SSLError):
        pass
    finally:
        writer.close()


async def read_head(reader):
    head = await reader.readuntil(b"\r\n\r\n")
    lines = head.decode("latin-1").split("\r\n")
    return head, lines[0], lines[1:]


async def main(listen_host, listen_port, proxy_host, proxy_port, ca_file):
    context = ssl.create_default_context(cafile=ca_file)

    async def connect_upstream(host, port):
        reader, writer = await asyncio.open_connection(proxy_host, proxy_port)
        writer.write(f"CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n".encode())
        await writer.drain()
        _, status, _ = await read_head(reader)
        if " 200" not in status:
            writer.close()
            raise OSError(status)
        return reader, writer

    async def handle(client_reader, client_writer):
        try:
            head, request, headers = await read_head(client_reader)
            method, target, version = request.split(" ", 2)
            if method == "CONNECT":
                upstream_reader, upstream_writer = await asyncio.open_connection(proxy_host, proxy_port)
                upstream_writer.write(head)
                await upstream_writer.drain()
                await asyncio.gather(pipe(client_reader, upstream_writer), pipe(upstream_reader, client_writer))
                return
            url = urlsplit(target)
            local = f"{url.hostname}:{url.port or 80}" in LOCAL_HOSTS
            if local:
                reader, writer = await asyncio.open_connection(url.hostname, url.port or 80)
            else:
                reader, writer = await connect_upstream(url.hostname, 443)
                await writer.start_tls(context, server_hostname=url.hostname)
            path = url.path or "/"
            if url.query:
                path += "?" + url.query
            kept = [h for h in headers if h and not h.lower().startswith(("proxy-", "connection:"))]
            request_head = f"{method} {path} {version}\r\n" + "".join(h + "\r\n" for h in kept)
            writer.write((request_head + "Connection: close\r\n\r\n").encode("latin-1"))
            await writer.drain()
            await pipe(reader, client_writer)
        except (OSError, ValueError, asyncio.IncompleteReadError, ssl.SSLError):
            client_writer.close()

    server = await asyncio.start_server(handle, listen_host, listen_port)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    host, port, proxy_host, proxy_port, ca_file = sys.argv[1:6]
    asyncio.run(main(host, int(port), proxy_host, int(proxy_port), ca_file))
