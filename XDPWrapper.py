import ctypes
import socket
import asyncio
import os
from ctypes.util import find_library

# Define sockaddr_xdp structure
class sockaddr_xdp(ctypes.Structure):
    _fields_ = [
        ("sxdp_family", ctypes.c_ushort),
        ("sxdp_flags", ctypes.c_uint),
        ("sxdp_ifindex", ctypes.c_uint),
        ("sxdp_queue_id", ctypes.c_uint),
        ("sxdp_shared_umem_fd", ctypes.c_uint64),
    ]

class XDPWrapper:
    def __init__(self, interface_name: str):
        self.interface_name = interface_name
        self.xdp_socket_fd = None
        # Use lib c to access most bash stuff in python
        self.libc = ctypes.CDLL(find_library("c"))
        # 44 is the AF XDP socket family
        self.AF_XDP = 44

    def setup_xdp_socket(self):
        """ Set up and bind the XDP socket to the specified interface. """
        print(f"Setting up XDP socket on interface: {self.interface_name}")
        # Create the XDP socket (AF_XDP and SOCK_RAW)
        self.xdp_socket_fd = self.libc.socket(self.AF_XDP, socket.SOCK_RAW, 0)
        if self.xdp_socket_fd < 0:
            raise OSError("Failed to create XDP socket")
        print(f"XDP socket created: FD = {self.xdp_socket_fd}")
        # Get the interface index
        try:
            ifindex = socket.if_nametoindex(self.interface_name)
            print(f"Interface {self.interface_name} has index: {ifindex}")
        except OSError:
            raise OSError(f"Failed to get interface index for {self.interface_name}")
        if ifindex <= 0:
            raise OSError(f"Invalid interface index {ifindex} for interface {self.interface_name}")
        # Try binding to available queue IDs
        for queue_id in range(5):  # Assuming queues 0 to 4
            print(f"Attempting to bind to queue_id = {queue_id}")
            sock_addr = sockaddr_xdp(
                sxdp_family=self.AF_XDP,
                sxdp_flags=0,  # Adjust if necessary
                sxdp_ifindex=ifindex,
                sxdp_queue_id=queue_id,
                sxdp_shared_umem_fd=0  # Not using shared UMEM
            )
            bind_result = self.libc.bind(self.xdp_socket_fd, ctypes.byref(sock_addr), ctypes.sizeof(sock_addr))
            if bind_result == 0:
                print(f"Successfully bound to queue_id = {queue_id}")
                self.queue_id = queue_id
                break
            else:
                err = ctypes.get_errno()
                print(f"Failed to bind to queue_id = {queue_id}, errno: {err}, message: {os.strerror(err)}")
        else:
            raise OSError("Failed to bind XDP socket to any queue")

        print(f"XDP socket successfully bound to interface {self.interface_name} on queue {self.queue_id}")

    async def receive_packets(self):
        '''Receive function, receives packets async.'''
        BUFFER_SIZE = 2048
        buffer = ctypes.create_string_buffer(BUFFER_SIZE)
        loop = asyncio.get_event_loop()
        while True:
            # Use run_in_executor to handle the blocking recv call
            try:
                num_bytes = await loop.run_in_executor(None, lambda: self.libc.recv(self.xdp_socket_fd, buffer, BUFFER_SIZE, 0))
                if num_bytes > 0:
                    packet = buffer.raw[:num_bytes]
                    print(f"Received packet of length {num_bytes}")
                    # Process the packet and handle it accordingly
                    await self.process_packet(packet)
                else:
                    break
            except OSError as e:
                print(f"Error receiving packet: {e}")
                break

    async def process_packet(self, packet):
        """Process the received packet and send it back if necessary."""
        print(f"Processing packet: {packet[:64].hex()}")
        await self.send_packet(packet)

    async def send_packet(self, packet):
        """Send the processed packet back to the sender via XDP socket."""
        loop = asyncio.get_event_loop()
        try:
            await loop.run_in_executor(None, lambda: self.libc.send(self.xdp_socket_fd, packet, len(packet), 0))
        except OSError as e:
            print(f"Error sending packet: {e}")

    def close(self):
        """Close the XDP socket."""
        if self.xdp_socket_fd:
            self.libc.close(self.xdp_socket_fd)
            self.xdp_socket_fd = None
            print("XDP socket closed")


async def main():
    xdp_wrapper = XDPWrapper("eno1")
    try:
        xdp_wrapper.setup_xdp_socket()
        await xdp_wrapper.receive_packets()
    finally:
        xdp_wrapper.close()

# Run the asyncio event loop
if __name__ == "__main__":
    asyncio.run(main())
