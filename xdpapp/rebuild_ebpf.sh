#!/bin/bash
clang -O2 -Wall -target bpf -c xdp_kernel.c -o xdp_kernel.o
ip link set eno1 xdpgeneric off
ip link set dev eno1 xdpgeneric obj xdp_kernel.o sec xdp_sock