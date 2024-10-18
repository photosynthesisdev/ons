import asyncio
import websockets
import ssl
import argparse
import socket

BIND_ADDRESS = '0.0.0.0'
BIND_PORT = 4040

async def handler(websocket, path, disable_nagling):
    if disable_nagling:
        transport = websocket.transport
        sock = transport.get_extra_info('socket')
        if sock is not None:
            try:
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                print("Nagle's algorithm disabled (TCP_NODELAY set).")
            except socket.error as e:
                print(f"Failed to set TCP_NODELAY: {e}")
        else:
            print("Socket is None; cannot set TCP_NODELAY.")
    else:
        print("Nagle's algorithm enabled (default behavior).")

    async for message in websocket:
        response_message = f"Received {message}"
        await websocket.send(response_message)

async def main(certfile=None, keyfile=None, port=BIND_PORT, disable_nagling=False):
    ssl_context = None
    if certfile and keyfile:
        ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ssl_context.load_cert_chain(certfile, keyfile)
        server_uri = f"wss://{BIND_ADDRESS}:{port}"
    else:
        server_uri = f"ws://{BIND_ADDRESS}:{port}"

    async def handler_wrapper(websocket, path):
        await handler(websocket, path, disable_nagling)

    async with websockets.serve(handler_wrapper, BIND_ADDRESS, port, ssl=ssl_context):
        print(f"Server listening on {server_uri}")
        await asyncio.Future()  # Run forever

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Simple WebSocket Server with Nagle's Algorithm Option")
    parser.add_argument(
        '--certificate', type=str, help="Path to the SSL certificate"
    )
    parser.add_argument(
        '--private-key', type=str, help="Path to the SSL private key"
    )
    parser.add_argument(
        '--port', type=int, default=BIND_PORT, help="Port to listen on"
    )
    parser.add_argument(
        '--disable-nagling', action='store_true', help="Disable Nagle's algorithm (enable TCP_NODELAY)"
    )
    args = parser.parse_args()
    asyncio.run(main(args.certificate, args.private_key, args.port, args.disable_nagling))
