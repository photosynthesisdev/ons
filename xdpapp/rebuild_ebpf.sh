#!/bin/bash

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 <protocol> <port> <loss_rate>"
    exit 1
fi

PROTOCOL=$1
PORT=$2
LOSS_RATE=$3

if [[ "$PROTOCOL" == "tcp" ]]; then
    PROTOCOL_VALUE=IPPROTO_TCP
elif [[ "$PROTOCOL" == "udp" ]]; then
    PROTOCOL_VALUE=IPPROTO_UDP
else
    echo "Invalid protocol. Use 'tcp' or 'udp'."
    exit 1
fi

cd /users/dorlando/ons/xdpapp

# Compile the XDP program with dynamic parameters
clang -O2 -Wall -target bpf \
    -DDROP_PROTOCOL=$PROTOCOL_VALUE \
    -DDROP_PORT=$PORT \
    -DDROP_RATE=$LOSS_RATE \
    -c xdp_kernel.c -o xdp_kernel.o

# Detach any existing XDP program and attach the new one
ip link set dev eno2 xdpgeneric off
ip link set dev eno2 xdpgeneric obj xdp_kernel.o sec xdp_sock
