#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/udp.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_endian.h>
#include <linux/in.h>

struct bpf_map_def SEC("maps") xsks_map = {
    .type = BPF_MAP_TYPE_XSKMAP,
    .key_size = sizeof(__u32),
    .value_size = sizeof(int),
    .max_entries = 64,
};

// Helper function to handle incoming UDP packets.
static __always_inline int handle_udp_packet(struct xdp_md *ctx, void *data, void *data_end) {
    struct ethhdr *eth = data;
    struct iphdr *ip = (data + sizeof(*eth));
    struct udphdr *udp = (void *)(ip + 1);

    if ((void *)(udp + 1) > data_end) {
        return XDP_PASS;
    }

    __u16 dest_port = bpf_ntohs(udp->dest);

    // If UDP destination port is 4433, check if XDP socket is active
    if (dest_port == 4433) {
        __u32 index = ctx->rx_queue_index;
        int *sock_fd = bpf_map_lookup_elem(&xsks_map, &index);
        // If no socket entry is found for this queue index, drop the packet
        if (!sock_fd) {
            return XDP_DROP;
        }
        // Redirect to user space if a valid XDP socket is available
        return bpf_redirect_map(&xsks_map, index, 0);
    }
    // For all other UDP packets, pass them to the kernel
    return XDP_PASS;
}

SEC("xdp_sock")
int xdp_sock_prog(struct xdp_md *ctx) {
    void *data = (void *)(long)ctx->data;
    void *data_end = (void *)(long)ctx->data_end;
    struct ethhdr *eth = data;
    // Check if it's an IP packet (EtherType == 0x0800)
    if ((void *)(eth + 1) > data_end) {
        return XDP_PASS;  // Invalid packet, pass to kernel
    }
    if (eth->h_proto != bpf_htons(ETH_P_IP)) {
        return XDP_PASS;  // Not an IP packet, pass to kernel
    }
    // Parse the IP header
    struct iphdr *ip = (data + sizeof(*eth));
    if ((void *)(ip + 1) > data_end) {
        return XDP_PASS;  // Invalid IP packet, pass to kernel
    }
    // Check if the packet is a UDP packet (IP protocol == 17)
    if (ip->protocol == IPPROTO_UDP) {
        return handle_udp_packet(ctx, data, data_end);
    }
    // For all non-UDP packets, pass them to the kernel
    return XDP_PASS;
}

char _license[] SEC("license") = "GPL";
