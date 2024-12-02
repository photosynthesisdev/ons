#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <linux/udp.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_endian.h>
#include <linux/in.h>

// Parameters provided dynamically via macros
#ifndef DROP_PROTOCOL
#define DROP_PROTOCOL IPPROTO_TCP
#endif

#ifndef DROP_PORT
#define DROP_PORT 4043
#endif

#ifndef DROP_RATE
#define DROP_RATE 1000
#endif

SEC("xdp_sock")
int xdp_filter_prog(struct xdp_md *ctx) {
    void *data = (void *)(long)ctx->data;
    void *data_end = (void *)(long)ctx->data_end;

    // Check Ethernet header
    struct ethhdr *eth = data;
    if ((void *)(eth + 1) > data_end) {
        return XDP_PASS;
    }

    // Verify if it’s an IP packet
    if (eth->h_proto != bpf_htons(ETH_P_IP)) {
        return XDP_PASS;
    }

    // Check IP header
    struct iphdr *ip = (void *)(eth + 1);
    if ((void *)(ip + 1) > data_end) {
        return XDP_PASS;
    }

    // Verify protocol
    if (ip->protocol != DROP_PROTOCOL) {
        return XDP_PASS;
    }

    // Check transport header and port
    if (ip->protocol == IPPROTO_TCP) {
        struct tcphdr *tcp = (struct tcphdr *)(ip + 1);
        if ((void *)(tcp + 1) > data_end) {
            return XDP_PASS;
        }

        if (bpf_ntohs(tcp->dest) == DROP_PORT) {
            // Drop packet based on DROP_RATE
            if (bpf_get_prandom_u32() % DROP_RATE == 0) {
                return XDP_DROP;
            }
        }
    } else if (ip->protocol == IPPROTO_UDP) {
        struct udphdr *udp = (struct udphdr *)(ip + 1);
        if ((void *)(udp + 1) > data_end) {
            return XDP_PASS;
        }

        if (bpf_ntohs(udp->dest) == DROP_PORT) {
            // Drop packet based on DROP_RATE
            if (bpf_get_prandom_u32() % DROP_RATE == 0) {
                return XDP_DROP;
            }
        }
    }

    return XDP_PASS;
}

// License
char _license[] SEC("license") = "GPL";
