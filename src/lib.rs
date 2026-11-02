#[cfg(test)]
mod tests {
    use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
    use smoltcp::phy::{Loopback, Medium};
    use smoltcp::socket::udp::Socket;
    use smoltcp::storage::{PacketBuffer, PacketMetadata};
    use smoltcp::time::Instant;
    use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint};

    const TX_CAPACITY: usize = 2048;
    // max UDP payload over a 1514-byte Ethernet frame: 1514 - 14 (eth) - 20 (IPv4) - 8 (UDP)
    const MAX_SIZE: usize = 1472;
    // anything above TX_CAPACITY - MAX_SIZE (576) and below MAX_SIZE triggers the bug
    const FIRST_SIZE: usize = 1000;

    const LOCAL_PORT: u16 = 9998;

    fn remote() -> IpEndpoint {
        IpEndpoint::new(IpAddress::v4(192, 168, 0, 4), 9999)
    }

    fn packet_buffer() -> PacketBuffer<'static, ()> {
        PacketBuffer::new(vec![PacketMetadata::EMPTY; 8], vec![0u8; TX_CAPACITY])
    }

    ////////////////////////////////////////////////////////////////////////////////
    struct Harness {
        device: Loopback,
        interface: Interface,
        sockets: SocketSet<'static>,
        handle: SocketHandle,
    }

    impl Harness {
        fn new() -> Self {
            let mut device = Loopback::new(Medium::Ip);
            let mut iface = Interface::new(Config::new(HardwareAddress::Ip), &mut device, Instant::ZERO);
            iface.update_ip_addrs(|addrs| {
                addrs.push(IpCidr::new(IpAddress::v4(192, 168, 0, 2), 24)).unwrap();
            });

            let rx = PacketBuffer::new(vec![PacketMetadata::EMPTY; 4], vec![0; 512]);
            let tx = PacketBuffer::new(vec![PacketMetadata::EMPTY; 8], vec![0; TX_CAPACITY]);
            let mut socket = Socket::new(rx, tx);
            socket.bind(LOCAL_PORT).unwrap();

            let mut sockets = SocketSet::new(vec![]);
            let handle = sockets.add(socket);

            Self { device, interface: iface, sockets, handle }
        }

        fn socket(&mut self) -> &mut Socket<'static> {
            self.sockets.get_mut::<Socket>(self.handle)
        }

        fn drain(&mut self) {
            self.interface.poll(Instant::ZERO, &mut self.device, &mut self.sockets);
            assert_eq!(self.socket().send_queue(), 0, "tx buffer should be empty after poll");
        }
    }

    ////////////////////////////////////////////////////////////////////////////////
    // socket level: the same two datagrams (FIRST_SIZE, then MAX_SIZE bytes) through each API

    #[test]
    fn send_slice_after_drain() {
        let mut h = Harness::new();

        h.socket().send_slice(&[0xAA; FIRST_SIZE], remote()).unwrap();
        h.drain();

        let result = h.socket().send_slice(&[0xBB; MAX_SIZE], remote());
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn send_with_after_drain() {
        let mut h = Harness::new();

        h.socket().send_with(MAX_SIZE, remote(), |_| FIRST_SIZE).unwrap();
        h.drain();

        let result = h.socket().send_with(MAX_SIZE, remote(), |_| MAX_SIZE);
        assert_eq!(result, Ok(MAX_SIZE), "send_with reported BufferFull on an empty tx buffer");
    }

    ////////////////////////////////////////////////////////////////////////////////
    // storage level: the same scenario directly on PacketBuffer, no interface involved

    #[test]
    fn enqueue_after_drain() {
        let mut buffer = packet_buffer();

        buffer.enqueue(FIRST_SIZE, ()).unwrap();
        buffer.dequeue().unwrap();
        assert!(buffer.is_empty());

        assert!(buffer.enqueue(MAX_SIZE, ()).is_ok());
    }

    #[test]
    fn enqueue_with_infallible_after_drain() {
        let mut buffer = packet_buffer();

        buffer.enqueue_with_infallible(MAX_SIZE, (), |_| FIRST_SIZE).unwrap();
        buffer.dequeue().unwrap();
        assert!(buffer.is_empty());

        let result = buffer.enqueue_with_infallible(MAX_SIZE, (), |_| MAX_SIZE);
        assert!(result.is_ok(), "enqueue_with_infallible reported Full on an empty buffer");
    }

    ////////////////////////////////////////////////////////////////////////////////
    // workaround: with max_size <= capacity / 2 an empty buffer always has room on one side of read_at

    #[test]
    fn enqueue_with_infallible_half_capacity_workaround() {
        let max_size = TX_CAPACITY / 2;

        for first_size in 1..=max_size {
            let mut buffer = packet_buffer();

            buffer.enqueue_with_infallible(max_size, (), |_| first_size).unwrap();
            buffer.dequeue().unwrap();

            let result = buffer.enqueue_with_infallible(max_size, (), |_| max_size);
            assert!(result.is_ok(), "first_size = {first_size}");
        }
    }
}
