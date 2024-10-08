#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/udp.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_endian.h>  // For bpf_htons and bpf_ntohs
#include <linux/in.h>        // For IPPROTO_UDP

struct bpf_map_def SEC("maps") xsk_map = {
    .type = BPF_MAP_TYPE_XSKMAP,
    .key_size = sizeof(int),
    .value_size = sizeof(int),
    .max_entries = 64,
};

SEC("xdp_sock")
int xdp_filter_prog(struct xdp_md *ctx) {
    void *data = (void *)(long)ctx->data;
    void *data_end = (void *)(long)ctx->data_end;
    struct ethhdr *eth = data;
    // Verify if the packet is large enough to contain an Ethernet header
    if (data + sizeof(*eth) > data_end) {
        return XDP_PASS;
    }
    // Check if it's an IP packet
    if (eth->h_proto != bpf_htons(ETH_P_IP)) {
        return XDP_PASS;
    }
    // Parse the IP header
    struct iphdr *ip = (data + sizeof(*eth));
    if (data + sizeof(*eth) + sizeof(*ip) > data_end) {
        return XDP_PASS;
    }
    // Check if it's a UDP packet
    if (ip->protocol != IPPROTO_UDP) {
        return XDP_PASS;
    }
    // Parse the UDP header
    struct udphdr *udp = (void *)(ip + 1);
    if ((void *)(udp + 1) > data_end) {
        return XDP_PASS;
    }
    // If the destination port is not 8080, pass the packet to the kernel
    if (bpf_ntohs(udp->dest) != 8080) {
        return XDP_PASS;
    }
    // Redirect the packet to the XDP socket
    int index = ctx->rx_queue_index;
    if (bpf_map_lookup_elem(&xsk_map, &index)){
        return bpf_redirect_map(&xsk_map, index, 0);
    }
    // If we can't redirect, just pass to normal kernel networking stack. 
    return XDP_PASS;
}

char _license[] SEC("license") = "GPL";
