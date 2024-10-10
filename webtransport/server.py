import argparse
import asyncio
import logging
from aioquic.asyncio import QuicConnectionProtocol, serve
from aioquic.h3.connection import H3_ALPN, H3Connection
from aioquic.h3.events import (
    DatagramReceived,
    HeadersReceived,
    H3Event,
    WebTransportStreamDataReceived,
)
from aioquic.quic.configuration import QuicConfiguration
from aioquic.quic.events import ProtocolNegotiated, QuicEvent

BIND_ADDRESS = '0.0.0.0'
BIND_PORT = 4433


class SessionHandler:
    def __init__(self, session_id, http, protocol):
        self._session_id = session_id
        self._http = http
        self._protocol = protocol

    def h3_event_received(self, event: H3Event):
        if isinstance(event, DatagramReceived):
            data = event.data.decode()
            #print(f"Received datagram: {data}")
            response_message = f"Received {data}"
            self._http.send_datagram(self._session_id, response_message.encode())
            #print(f"Sent response datagram: {response_message}")
            self._protocol.transmit()  # Ensure data is sent
        elif isinstance(event, WebTransportStreamDataReceived):
            # Handle stream data if needed
            pass


class SimpleWebTransportServerProtocol(QuicConnectionProtocol):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._session_id = None
        self._http = None
        self._handler = None

    def quic_event_received(self, event: QuicEvent):
        #logging.debug(f"QUIC EVENT RECEIVED: {event}")
        if isinstance(event, ProtocolNegotiated):
            self._http = H3Connection(self._quic, enable_webtransport=True)

        if self._http:
            for http_event in self._http.handle_event(event):
                self.handle_http_event(http_event)

        self.transmit()

    def handle_http_event(self, event: H3Event):
        #logging.debug(f"HTTP EVENT RECEIVED: {event}")
        if isinstance(event, HeadersReceived):
            headers = dict(event.headers)
            method = headers.get(b':method')
            protocol = headers.get(b':protocol')

            if method == b'CONNECT' and protocol == b'webtransport':
                # Accept WebTransport session
                self._http.send_headers(
                    event.stream_id,
                    [
                        (b':status', b'200'),
                        (b'sec-webtransport-http3-draft', b'draft02'),
                    ],
                )
                self._session_id = event.stream_id
                self._handler = SessionHandler(self._session_id, self._http, self)
            else:
                # Reject other requests
                self._http.send_headers(
                    event.stream_id, [(b':status', b'404')], end_stream=True
                )
        else:
            if self._handler:
                self._handler.h3_event_received(event)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description="Simple WebTransport Server")
    parser.add_argument(
        '--certificate', type=str, required=True, help="Path to the SSL certificate"
    )
    parser.add_argument(
        '--private-key', type=str, required=True, help="Path to the SSL private key"
    )
    args = parser.parse_args()

    configuration = QuicConfiguration(
        is_client=False,
        alpn_protocols=H3_ALPN,
        max_datagram_frame_size=65536,
    )
    configuration.load_cert_chain(args.certificate, args.private_key)

    loop = asyncio.get_event_loop()
    server = loop.run_until_complete(
        serve(
            BIND_ADDRESS,
            BIND_PORT,
            configuration=configuration,
            create_protocol=SimpleWebTransportServerProtocol,
        )
    )

    try:
        print(f"Server listening on https://{BIND_ADDRESS}:{BIND_PORT}")
        loop.run_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.close()