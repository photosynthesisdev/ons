import asyncio
import websockets
import time
import argparse
import ssl
import socket

class Statistics:
    def __init__(self, ewma_alpha=0.1):
        self.message_count = 0
        self.start_time = time.time()
        self.rtt_samples = []
        self.ewma_rtt = None
        self.ewma_alpha = ewma_alpha

    def record_message(self):
        self.message_count += 1

    def messages_per_second(self):
        elapsed_time = time.time() - self.start_time
        return self.message_count / elapsed_time if elapsed_time > 0 else 0

    def record_rtt(self, rtt_ms):
        self.rtt_samples.append(rtt_ms)
        if self.ewma_rtt is None:
            self.ewma_rtt = rtt_ms
        else:
            self.ewma_rtt = (self.ewma_alpha * rtt_ms) + ((1 - self.ewma_alpha) * self.ewma_rtt)

    def get_ewma_rtt(self):
        return self.ewma_rtt

    def print_statistics(self):
        print(f"Total messages sent: {self.message_count}")
        print(f"Messages per second: {self.messages_per_second():.2f}")
        if self.rtt_samples:
            average_rtt = sum(self.rtt_samples) / len(self.rtt_samples)
            print(f"Average RTT: {average_rtt:.2f} ms")
            print(f"EWMA RTT: {self.get_ewma_rtt():.2f} ms")

async def run(uri, message_limit, ssl_context, disable_nagling):
    async with websockets.connect(uri, ssl=ssl_context) as websocket:
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

        statistics = Statistics()
        for counter in range(message_limit):
            message = f"Message {counter}"
            send_time = time.time()
            await websocket.send(message)
            # Wait for response
            try:
                response = await websocket.recv()
            except websockets.exceptions.ConnectionClosed:
                print("Connection closed by the server.")
                break
            recv_time = time.time()
            rtt_ms = (recv_time - send_time) * 1000
            statistics.record_rtt(rtt_ms)
            statistics.record_message()
        statistics.print_statistics()

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Simple WebSocket Client with Nagle's Algorithm Option")
    parser.add_argument("host", type=str, help="Server hostname or IP")
    parser.add_argument("port", type=int, help="Server port")
    parser.add_argument("--count", type=int, default=10, help="Number of messages to send")
    parser.add_argument(
        '--no-ssl', action='store_true', help="Use ws:// instead of wss://"
    )
    parser.add_argument(
        '--insecure', action='store_true', help="Disable SSL certificate verification"
    )
    parser.add_argument(
        '--disable-nagling', action='store_true', help="Disable Nagle's algorithm (enable TCP_NODELAY)"
    )
    args = parser.parse_args()
    if args.no_ssl:
        uri = f"ws://{args.host}:{args.port}"
        ssl_context = None
    else:
        uri = f"wss://{args.host}:{args.port}"
        ssl_context = ssl.create_default_context()
        if args.insecure:
            ssl_context.check_hostname = False
            ssl_context.verify_mode = ssl.CERT_NONE
    asyncio.run(run(uri, args.count, ssl_context, args.disable_nagling))
