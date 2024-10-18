import argparse
import asyncio
import time
from aioquic.asyncio.client import connect
from aioquic.h3.connection import H3_ALPN
from aioquic.h3.events import DatagramReceived, HeadersReceived, H3Event
from aioquic.quic.configuration import QuicConfiguration
from aioquic.asyncio import QuicConnectionProtocol
from aioquic.h3.connection import H3Connection
import logging
import ssl
import binascii
 
class Statistics:
    def __init__(self, ewma_alpha = 0.1):
        self.message_count = 0
        self.start_time = time.time()
        self.last_time = self.start_time
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

class SimpleWebTransportClientProtocol(QuicConnectionProtocol):
    def __init__(self, message_limit, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._http = None
        self._session_id = None
        self._session_established = asyncio.Event()
        self._datagram_received = asyncio.Event()
        self._counter = 0
        self._message_limit = message_limit
        self._statistics = Statistics()
        self._send_timestamps = {}

    def quic_event_received(self, event):
        if self._http:
            for http_event in self._http.handle_event(event):
                self.handle_http_event(http_event)
        self.transmit()

    def handle_http_event(self, event):
        if isinstance(event, HeadersReceived):
            headers = dict(event.headers)
            status = headers.get(b':status')
            if status == b'200':
                print("WebTransport session established.")
                self._session_established.set()
            else:
                print(f"Failed to establish WebTransport session. Status: {status.decode()}")
        elif isinstance(event, DatagramReceived):
            recv_time = time.time()
            data = event.data.decode()
            #print(f"Received datagram: {data}")
            message_id = int(data.split()[-1])
            if message_id in self._send_timestamps:
                send_time = self._send_timestamps.pop(message_id)
                rtt_ms = (recv_time - send_time) * 1000
                self._statistics.record_rtt(rtt_ms)
                #print(f"RTT for message {message_id}: {rtt_ms:.2f} ms")
            self._datagram_received.set()

    async def send_datagram(self):
        await self._session_established.wait()
        while self._counter < self._message_limit:
            message = f"Message {self._counter}"
            self._http.send_datagram(self._session_id, message.encode())
            self._send_timestamps[self._counter] = time.time()
            #print(f"Sent datagram: {message}")
            self.transmit()
            self._counter += 1
            self._statistics.record_message()
            await self._datagram_received.wait()
            self._datagram_received.clear()
        self._statistics.print_statistics()
        self._quic.close()

async def run(host, port, message_limit):
    configuration = QuicConfiguration(
        is_client=True,
        alpn_protocols=H3_ALPN,
        max_datagram_frame_size=65536,
    )
    configuration.verify_mode = ssl.CERT_NONE

    async with connect(
        host,
        port,
        configuration=configuration,
        create_protocol=lambda *args, **kwargs: SimpleWebTransportClientProtocol(message_limit, *args, **kwargs),
        session_ticket_handler=None,
    ) as client:
        http = H3Connection(client._quic, enable_webtransport=True)
        stream_id = client._quic.get_next_available_stream_id(is_unidirectional=False)
        headers = [
            (b":method", b"CONNECT"),
            (b":scheme", b"https"),
            (b":authority", f"{host}:{port}".encode()),
            (b":path", b"/counter"),
            (b":protocol", b'webtransport'),
        ]
        http.send_headers(stream_id, headers)
        client._session_id = stream_id
        client._http = http
        await client._session_established.wait()
        await client.send_datagram()

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Simple WebTransport Client")
    parser.add_argument("host", type=str, help="Server hostname or IP")
    parser.add_argument("port", type=int, help="Server port")
    parser.add_argument("--count", type=int, default=10, help="Number of messages to send")
    args = parser.parse_args()
    asyncio.run(run(args.host, args.port, args.count))