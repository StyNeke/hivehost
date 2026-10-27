#!/usr/bin/env python3
"""Test-only HTTPS terminator for local acceptance runs.

The API listens on loopback HTTP and expects a private HTTPS proxy in front (secure cookies
and an exact https:// origin). This terminates TLS with the given certificate and forwards
raw bytes to the API. Production should use a real reverse proxy and a trusted certificate.

usage: tls-proxy.py LISTEN_HOST LISTEN_PORT TARGET_HOST TARGET_PORT CERT_FILE KEY_FILE
"""
import asyncio
import ssl
import sys


async def pipe(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, ssl.SSLError):
        pass
    finally:
        writer.close()


async def main(host, port, target_host, target_port, cert, key):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(cert, key)

    async def handle(client_reader, client_writer):
        try:
            upstream_reader, upstream_writer = await asyncio.open_connection(target_host, target_port)
        except OSError:
            client_writer.close()
            return
        await asyncio.gather(pipe(client_reader, upstream_writer), pipe(upstream_reader, client_writer))

    server = await asyncio.start_server(handle, host, port, ssl=context)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    host, port, target_host, target_port, cert, key = sys.argv[1:7]
    asyncio.run(main(host, int(port), target_host, int(target_port), cert, key))
