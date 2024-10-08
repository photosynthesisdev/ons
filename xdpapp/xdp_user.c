#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <fcntl.h>
#include <unistd.h>
#include <net/if.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <poll.h>
#include <linux/if_link.h>
#include <linux/if_xdp.h>
#include <linux/bpf.h>

#include <bpf/bpf.h>
#include <bpf/libbpf.h>
#include <bpf/xsk.h>

#define NUM_FRAMES 4096
#define FRAME_SIZE 2048
#define BATCH_SIZE 64

struct xsk_umem_info {
    struct xsk_ring_prod fill_q;
    struct xsk_ring_cons comp_q;
    struct xsk_umem *umem;
    void *buffer;
};

struct xsk_socket_info {
    struct xsk_ring_cons rx_q;
    struct xsk_ring_prod tx_q;
    struct xsk_umem_info *umem;
    struct xsk_socket *xsk;
};

int main(int argc, char **argv) {
    const char *ifname = "eno1";
    const char *prog_filename = "xdp_kernel.o";
    const char *prog_section = "xdp_sock";
    struct bpf_object *bpf_obj = NULL;
    struct bpf_program *bpf_prog = NULL;
    int ifindex, prog_fd, map_fd, ret;
    struct xsk_umem_info *umem;
    struct xsk_socket_info *xsk_info;
    struct xsk_umem_config umem_cfg;
    struct xsk_socket_config xsk_cfg;
    void *umem_buffer;
    uint32_t idx;
    // Just connect to RX queue 0 on eno1
    __u32 key = 0;
    int fd;
    unsigned int rcvd, i;
    uint32_t idx_rx = 0, idx_fq = 0;

    // Get eno1 interface index
    ifindex = if_nametoindex(ifname);
    if (!ifindex) {
        fprintf(stderr, "Failed to get interface index for %s\n", ifname);
        return -1;
    }

    // Load BPF object
    bpf_obj = bpf_object__open_file(prog_filename, NULL);
    if (libbpf_get_error(bpf_obj)) {
        fprintf(stderr, "Failed to open BPF object file: %s\n", prog_filename);
        return -1;
    }

    // Load the BPF program
    ret = bpf_object__load(bpf_obj);
    if (ret) {
        fprintf(stderr, "Failed to load BPF object: %s\n", strerror(-ret));
        bpf_object__close(bpf_obj);
        return -1;
    }

    // Find the specific program by section name
    bpf_prog = bpf_object__find_program_by_title(bpf_obj, prog_section);
    if (!bpf_prog) {
        fprintf(stderr, "Failed to find program in section: %s\n", prog_section);
        bpf_object__close(bpf_obj);
        return -1;
    }

    // Get program FD
    prog_fd = bpf_program__fd(bpf_prog);

    // Get xsk_map FD before attaching the program
    map_fd = bpf_object__find_map_fd_by_name(bpf_obj, "xsk_map");
    if (map_fd < 0) {
        fprintf(stderr, "Failed to find xsk_map in BPF object\n");
        bpf_object__close(bpf_obj);
        return -1;
    }

    // --- UMEM SETUP, taken from Kernel Docs ---
    umem = calloc(1, sizeof(*umem));
    if (!umem) {
        fprintf(stderr, "Failed to allocate memory for UMEM\n");
        return -1;
    }

    ret = posix_memalign(&umem_buffer, getpagesize(), NUM_FRAMES * FRAME_SIZE);
    if (ret) {
        fprintf(stderr, "Failed to allocate UMEM buffer: %s\n", strerror(ret));
        free(umem);
        return -1;
    }

    umem_cfg.fill_size = NUM_FRAMES;
    umem_cfg.comp_size = NUM_FRAMES;
    umem_cfg.frame_size = FRAME_SIZE;
    umem_cfg.frame_headroom = 0;
    umem_cfg.flags = 0;

    ret = xsk_umem__create(&umem->umem, umem_buffer, NUM_FRAMES * FRAME_SIZE, &umem->fill_q, &umem->comp_q, &umem_cfg);
    if (ret) {
        fprintf(stderr, "Failed to create UMEM: %s\n", strerror(-ret));
        free(umem_buffer);
        free(umem);
        return ret;
    }
    umem->buffer = umem_buffer;

    // --- SETUP XSK SOCKET ----
    xsk_info = calloc(1, sizeof(*xsk_info));
    if (!xsk_info) {
        fprintf(stderr, "Failed to allocate memory for xsk_info\n");
        xsk_umem__delete(umem->umem);
        free(umem_buffer);
        free(umem);
        return -1;
    }

    xsk_cfg.rx_size = NUM_FRAMES;
    xsk_cfg.tx_size = NUM_FRAMES;
    xsk_cfg.libbpf_flags = 0;
    xsk_cfg.xdp_flags = XDP_FLAGS_SKB_MODE; // Using generic (SKB) mode
    xsk_cfg.bind_flags = 0;

    ret = xsk_socket__create(&xsk_info->xsk, ifname, key, umem->umem, &xsk_info->rx_q, &xsk_info->tx_q, &xsk_cfg);
    if (ret) {
        fprintf(stderr, "Failed to create XSK socket: %s\n", strerror(-ret));
        xsk_umem__delete(umem->umem);
        free(umem_buffer);
        free(umem);
        free(xsk_info);
        return ret;
    }
    xsk_info->umem = umem;

    // --- UPDATE xsk_map BEFORE ATTACHING THE XDP PROGRAM ---
    fd = xsk_socket__fd(xsk_info->xsk);
    ret = bpf_map_update_elem(map_fd, &key, &fd, 0);
    if (ret) {
        fprintf(stderr, "Failed to update xsk_map: %s\n", strerror(errno));
        xsk_socket__delete(xsk_info->xsk);
        xsk_umem__delete(umem->umem);
        free(umem_buffer);
        free(umem);
        free(xsk_info);
        return -1;
    }

    // --- ATTACH XDP PROGRAM TO THE INTERFACE ---
    ret = bpf_set_link_xdp_fd(ifindex, prog_fd, XDP_FLAGS_SKB_MODE);
    if (ret < 0) {
        fprintf(stderr, "Failed to attach XDP program: %s\n", strerror(-ret));
        xsk_socket__delete(xsk_info->xsk);
        xsk_umem__delete(umem->umem);
        free(umem_buffer);
        free(umem);
        free(xsk_info);
        return -1;
    }

    // --- POPULATE FILL RING ---
    ret = xsk_ring_prod__reserve(&umem->fill_q, NUM_FRAMES, &idx);
    if (ret != NUM_FRAMES) {
        fprintf(stderr, "Failed to reserve fill ring slots\n");
        // Cleanup
        bpf_set_link_xdp_fd(ifindex, -1, XDP_FLAGS_SKB_MODE);
        xsk_socket__delete(xsk_info->xsk);
        xsk_umem__delete(umem->umem);
        free(umem_buffer);
        free(umem);
        free(xsk_info);
        return -1;
    }
    for (i = 0; i < NUM_FRAMES; i++) {
        *xsk_ring_prod__fill_addr(&umem->fill_q, idx + i) = i * FRAME_SIZE;
    }
    xsk_ring_prod__submit(&umem->fill_q, NUM_FRAMES);

    // --- START PROCESSING PACKETS ---
    struct pollfd fds[1];
    fds[0].fd = xsk_socket__fd(xsk_info->xsk);
    fds[0].events = POLLIN;

    printf("Starting packet processing loop...\n");
    while (1) {
        ret = poll(fds, 1, -1);
        if (ret <= 0) {
            perror("poll");
            continue;
        }

        // Receive packets from the kernel
        rcvd = xsk_ring_cons__peek(&xsk_info->rx_q, BATCH_SIZE, &idx_rx);
        if (!rcvd)
            continue;

        // Process each received packet
        for (i = 0; i < rcvd; i++) {
            const struct xdp_desc *desc = xsk_ring_cons__rx_desc(&xsk_info->rx_q, idx_rx + i);
            uint64_t addr = desc->addr;
            uint32_t len = desc->len;

            // Get packet data
            void *pkt = xsk_umem__get_data(umem->buffer, addr);
            if (!pkt) {
                fprintf(stderr, "Failed to get packet data\n");
                continue;
            }

            printf("Received packet of length %u bytes\n", len);

            // Example: Print first 32 bytes of packet data
            for (uint32_t j = 0; j < len && j < 32; j++) {
                printf("%02x ", ((unsigned char *)pkt)[j]);
            }
            printf("\n");
        }

        // Release the descriptors back to the kernel
        xsk_ring_cons__release(&xsk_info->rx_q, rcvd);

        // Refill the fill ring with used descriptors
        ret = xsk_ring_prod__reserve(&umem->fill_q, rcvd, &idx_fq);
        if (ret != rcvd) {
            fprintf(stderr, "Failed to reserve fill ring slots for refill\n");
            continue;
        }
        for (i = 0; i < rcvd; i++) {
            const struct xdp_desc *desc = xsk_ring_cons__rx_desc(&xsk_info->rx_q, idx_rx + i);
            *xsk_ring_prod__fill_addr(&umem->fill_q, idx_fq + i) = desc->addr;
        }
        xsk_ring_prod__submit(&umem->fill_q, rcvd);
    }

    // --- CLEANUP (Unreachable in this example, but good practice) ---
    xsk_socket__delete(xsk_info->xsk);
    xsk_umem__delete(umem->umem);
    bpf_set_link_xdp_fd(ifindex, -1, XDP_FLAGS_SKB_MODE);
    bpf_object__close(bpf_obj);
    free(umem->buffer);
    free(umem);
    free(xsk_info);

    return 0;
}
