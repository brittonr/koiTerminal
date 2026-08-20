// Copyright 2024 The Android Open Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Copied from ChromiumOS with relicensing:
// src/platform2/vm_tools/chunnel/src/bin/chunneld.rs

//! Host-side stream socket forwarder

use std::collections::btree_map::Entry as BTreeMapEntry;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;
use std::result;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use forwarder::forwarder::ForwarderSession;
use jni::objects::{JIntArray, JObject, JString, JValue};
use jni::sys::jint;
use jni::JNIEnv;
use log::{debug, error, info, warn};
use nix::sys::eventfd::{EfdFlags, EventFd};
use poll_token_derive::PollToken;
use vmm_sys_util::poll::{PollContext, PollToken};
use vsock::VsockListener;
use vsock::VMADDR_CID_ANY;

const CHUNNEL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_FORWARDED_PORTS: usize = 16;
const MIN_FORWARDED_PORT: u16 = 1024;

const VMADDR_PORT_ANY: u32 = u32::MAX;

static SHUTDOWN_EVT: LazyLock<EventFd> =
    LazyLock::new(|| EventFd::new().expect("Could not create shutdown eventfd"));

static UPDATE_EVT: LazyLock<EventFd> = LazyLock::new(|| {
    EventFd::from_flags(EfdFlags::EFD_SEMAPHORE).expect("Could not create update eventfd")
});

static UPDATE_QUEUE: LazyLock<Arc<Mutex<VecDeque<u16>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(VecDeque::new())));

static DIRECT_SHUTDOWN_EVT: LazyLock<EventFd> =
    LazyLock::new(|| EventFd::new().expect("Could not create direct shutdown eventfd"));

static DIRECT_UPDATE_EVT: LazyLock<EventFd> = LazyLock::new(|| {
    EventFd::from_flags(EfdFlags::EFD_SEMAPHORE | EfdFlags::EFD_NONBLOCK)
        .expect("Could not create direct update eventfd")
});

static DIRECT_UPDATE_QUEUE: LazyLock<Arc<Mutex<VecDeque<u16>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(VecDeque::new())));

#[remain::sorted]
#[derive(Debug)]
enum Error {
    BindVsock(io::Error),
    IncorrectCid(u32),
    LaunchForwarderGuest(jni::errors::Error),
    NoListenerForPort(u16),
    NoSessionForTag(SessionTag),
    PollContextAdd(vmm_sys_util::errno::Error),
    PollContextDelete(vmm_sys_util::errno::Error),
    PollContextNew(vmm_sys_util::errno::Error),
    PollWait(vmm_sys_util::errno::Error),
    SetVsockNonblocking(io::Error),
    TcpAccept(io::Error),
    TcpConnect(io::Error),
    TcpListenerPort(io::Error),
    UpdateEventRead(nix::Error),
    VsockAccept(io::Error),
    VsockAcceptTimeout,
    VsockListenerPort(io::Error),
}

type Result<T> = result::Result<T, Error>;

impl fmt::Display for Error {
    #[remain::check]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::Error::*;

        #[remain::sorted]
        match self {
            BindVsock(e) => write!(f, "failed to bind vsock: {e}"),
            IncorrectCid(cid) => write!(f, "chunnel connection from unexpected cid {cid}"),
            LaunchForwarderGuest(e) => write!(f, "failed to launch forwarder_guest {e}"),
            NoListenerForPort(port) => write!(f, "could not find listener for port: {port}"),
            NoSessionForTag(tag) => write!(f, "could not find session for tag: {tag:x}"),
            PollContextAdd(e) => write!(f, "failed to add fd to poll context: {e}"),
            PollContextDelete(e) => write!(f, "failed to delete fd from poll context: {e}"),
            PollContextNew(e) => write!(f, "failed to create poll context: {e}"),
            PollWait(e) => write!(f, "failed to wait for poll: {e}"),
            SetVsockNonblocking(e) => write!(f, "failed to set vsock to nonblocking: {e}"),
            TcpAccept(e) => write!(f, "failed to accept tcp: {e}"),
            TcpConnect(e) => write!(f, "failed to connect to guest tcp: {e}"),
            TcpListenerPort(e) => {
                write!(f, "failed to read local sockaddr for tcp listener: {e}")
            }
            UpdateEventRead(e) => write!(f, "failed to read update eventfd: {e}"),
            VsockAccept(e) => write!(f, "failed to accept vsock: {e}"),
            VsockAcceptTimeout => write!(f, "timed out waiting for vsock connection"),
            VsockListenerPort(e) => write!(f, "failed to get vsock listener port: {e}"),
        }
    }
}

/// A tag that uniquely identifies a particular forwarding session. This has arbitrarily been
/// chosen as the fd of the local (TCP) socket.
type SessionTag = u32;

/// Implements PollToken for chunneld's main poll loop.
#[derive(Clone, Copy, PollToken)]
enum Token {
    Shutdown,
    UpdatePorts,
    Ipv4Listener(u16),
    Ipv6Listener(u16),
    LocalSocket(SessionTag),
    RemoteSocket(SessionTag),
}

/// PortListeners includes all listeners (IPv4 and IPv6) for a given port, and the target
/// container.
struct PortListeners {
    tcp4_listener: TcpListener,
    tcp6_listener: TcpListener,
}

/// SocketFamily specifies whether a socket uses IPv4 or IPv6.
enum SocketFamily {
    Ipv4,
    Ipv6,
}

enum RemoteTarget<'a> {
    Direct(IpAddr),
    Vsock { cid: u32, jni_env: JNIEnv<'a>, jni_cb: JObject<'a> },
}

/// ForwarderSessions encapsulates all forwarding state for chunneld.
struct ForwarderSessions<'a> {
    listening_ports: BTreeMap<u16, PortListeners>,
    tcp4_forwarders: HashMap<SessionTag, ForwarderSession>,
    remote_target: RemoteTarget<'a>,
    shutdown_evt: &'static EventFd,
    update_evt: &'static EventFd,
    update_queue: &'static Arc<Mutex<VecDeque<u16>>>,
}

impl<'a> ForwarderSessions<'a> {
    fn new_vsock(cid: i32, jni_env: JNIEnv<'a>, jni_cb: JObject<'a>) -> Self {
        Self::new(
            RemoteTarget::Vsock { cid: cid as u32, jni_env, jni_cb },
            &SHUTDOWN_EVT,
            &UPDATE_EVT,
            &UPDATE_QUEUE,
        )
    }

    fn new_direct(guest_address: IpAddr) -> Self {
        Self::new(
            RemoteTarget::Direct(guest_address),
            &DIRECT_SHUTDOWN_EVT,
            &DIRECT_UPDATE_EVT,
            &DIRECT_UPDATE_QUEUE,
        )
    }

    fn new(
        remote_target: RemoteTarget<'a>,
        shutdown_evt: &'static EventFd,
        update_evt: &'static EventFd,
        update_queue: &'static Arc<Mutex<VecDeque<u16>>>,
    ) -> Self {
        ForwarderSessions {
            listening_ports: BTreeMap::new(),
            tcp4_forwarders: HashMap::new(),
            remote_target,
            shutdown_evt,
            update_evt,
            update_queue,
        }
    }

    /// Adds or removes listeners based on the latest listening ports from the D-Bus thread.
    fn process_update_queue(&mut self, poll_ctx: &PollContext<Token>) -> Result<()> {
        // Unwrap of LockResult is customary.
        let mut update_queue = self.update_queue.lock().unwrap();
        let mut active_ports: BTreeSet<u16> = BTreeSet::new();

        // Add any new listeners first.
        while let Some(port) = update_queue.pop_front() {
            // Ignore privileged ports.
            if port < 1024 {
                continue;
            }
            if let BTreeMapEntry::Vacant(o) = self.listening_ports.entry(port) {
                // Failing to bind a port is not fatal, but we should log it.
                // Both IPv4 and IPv6 localhost must be bound since the host may resolve
                // "localhost" to either.
                let tcp4_listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
                    Ok(listener) => listener,
                    Err(e) => {
                        warn!("failed to bind TCPv4 port: {e}");
                        continue;
                    }
                };
                let tcp6_listener = match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                    Ok(listener) => listener,
                    Err(e) => {
                        warn!("failed to bind TCPv6 port: {e}");
                        continue;
                    }
                };
                poll_ctx
                    .add(&tcp4_listener, Token::Ipv4Listener(port))
                    .map_err(Error::PollContextAdd)?;
                poll_ctx
                    .add(&tcp6_listener, Token::Ipv6Listener(port))
                    .map_err(Error::PollContextAdd)?;
                o.insert(PortListeners { tcp4_listener, tcp6_listener });
            }
            active_ports.insert(port);
        }

        // Iterate over the existing listeners; if the port is no longer in the
        // listener list, remove it.
        let old_ports: Vec<u16> = self.listening_ports.keys().cloned().collect();
        for port in old_ports.iter() {
            if !active_ports.contains(port) {
                // Remove the PortListeners struct first - on error we want to drop it and the
                // fds it contains.
                let _listening_port = self.listening_ports.remove(port);
            }
        }

        // Consume the eventfd.
        self.update_evt.read().map_err(Error::UpdateEventRead)?;

        Ok(())
    }

    fn accept_connection(
        &mut self,
        poll_ctx: &PollContext<Token>,
        port: u16,
        sock_family: SocketFamily,
    ) -> Result<()> {
        let port_listeners =
            self.listening_ports.get(&port).ok_or(Error::NoListenerForPort(port))?;

        let listener = match sock_family {
            SocketFamily::Ipv4 => &port_listeners.tcp4_listener,
            SocketFamily::Ipv6 => &port_listeners.tcp6_listener,
        };

        // This session should be dropped if any of the PollContext setup fails. Since the only
        // extant fds for the underlying sockets will be closed, they will be unregistered from
        // epoll set automatically.
        let session = match &mut self.remote_target {
            RemoteTarget::Direct(guest_address) => {
                create_direct_forwarder_session(listener, *guest_address)?
            }
            RemoteTarget::Vsock { cid, jni_env, jni_cb } => {
                create_vsock_forwarder_session(listener, *cid, jni_env, jni_cb)?
            }
        };

        let tag = session.local_stream().as_raw_fd() as u32;

        poll_ctx
            .add(session.local_stream(), Token::LocalSocket(tag))
            .map_err(Error::PollContextAdd)?;
        poll_ctx
            .add(session.remote_stream(), Token::RemoteSocket(tag))
            .map_err(Error::PollContextAdd)?;

        self.tcp4_forwarders.insert(tag, session);

        Ok(())
    }

    fn forward_from_local(&mut self, poll_ctx: &PollContext<Token>, tag: SessionTag) -> Result<()> {
        let session = self.tcp4_forwarders.get_mut(&tag).ok_or(Error::NoSessionForTag(tag))?;
        let shutdown = session.forward_from_local().unwrap_or(true);
        if shutdown {
            poll_ctx.delete(session.local_stream()).map_err(Error::PollContextDelete)?;
            if session.is_shut_down() {
                self.tcp4_forwarders.remove(&tag);
            }
        }

        Ok(())
    }

    fn forward_from_remote(
        &mut self,
        poll_ctx: &PollContext<Token>,
        tag: SessionTag,
    ) -> Result<()> {
        let session = self.tcp4_forwarders.get_mut(&tag).ok_or(Error::NoSessionForTag(tag))?;
        let shutdown = session.forward_from_remote().unwrap_or(true);
        if shutdown {
            poll_ctx.delete(session.remote_stream()).map_err(Error::PollContextDelete)?;
            if session.is_shut_down() {
                self.tcp4_forwarders.remove(&tag);
            }
        }

        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        let poll_ctx: PollContext<Token> = PollContext::new().map_err(Error::PollContextNew)?;
        poll_ctx.add(self.update_evt, Token::UpdatePorts).map_err(Error::PollContextAdd)?;
        poll_ctx.add(self.shutdown_evt, Token::Shutdown).map_err(Error::PollContextAdd)?;

        loop {
            let events = poll_ctx.wait().map_err(Error::PollWait)?;

            for event in events.iter_readable() {
                match event.token() {
                    Token::Shutdown => {
                        self.shutdown_evt.read().map_err(Error::UpdateEventRead)?;
                        return Ok(());
                    }
                    Token::UpdatePorts => {
                        if let Err(e) = self.process_update_queue(&poll_ctx) {
                            error!("error updating listening ports: {e}");
                        }
                    }
                    Token::Ipv4Listener(port) => {
                        if let Err(e) = self.accept_connection(&poll_ctx, port, SocketFamily::Ipv4)
                        {
                            error!("error accepting connection: {e}");
                        }
                    }
                    Token::Ipv6Listener(port) => {
                        if let Err(e) = self.accept_connection(&poll_ctx, port, SocketFamily::Ipv6)
                        {
                            error!("error accepting connection: {e}");
                        }
                    }
                    Token::LocalSocket(tag) => {
                        if let Err(e) = self.forward_from_local(&poll_ctx, tag) {
                            error!("error forwarding local traffic: {e}");
                        }
                    }
                    Token::RemoteSocket(tag) => {
                        if let Err(e) = self.forward_from_remote(&poll_ctx, tag) {
                            error!("error forwarding remote traffic: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Creates a forwarder session from a `listener` that has a pending connection to accept.
fn create_vsock_forwarder_session(
    listener: &TcpListener,
    cid: u32,
    jni_env: &mut JNIEnv,
    jni_cb: &JObject,
) -> Result<ForwarderSession> {
    let (tcp_stream, _) = listener.accept().map_err(Error::TcpAccept)?;
    // Bind a vsock port, tell the guest to connect, and accept the connection.
    let vsock_listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, VMADDR_PORT_ANY)
        .map_err(Error::BindVsock)?;
    vsock_listener.set_nonblocking(true).map_err(Error::SetVsockNonblocking)?;

    let tcp4_port = listener.local_addr().map_err(Error::TcpListenerPort)?.port();
    let vsock_port = vsock_listener.local_addr().map_err(Error::VsockListenerPort)?.port();
    jni_env
        .call_method(
            jni_cb,
            "onForwardingRequestReceived",
            "(II)V",
            &[JValue::Int(tcp4_port.into()), JValue::Int(vsock_port as i32)],
        )
        .map_err(Error::LaunchForwarderGuest)?;

    #[derive(PollToken)]
    enum Token {
        VsockAccept,
    }

    let poll_ctx: PollContext<Token> = PollContext::new().map_err(Error::PollContextNew)?;
    poll_ctx.add(&vsock_listener, Token::VsockAccept).map_err(Error::PollContextAdd)?;

    // Wait a few seconds for the guest to connect.
    let events = poll_ctx.wait_timeout(CHUNNEL_CONNECT_TIMEOUT).map_err(Error::PollWait)?;

    match events.iter_readable().next() {
        Some(_) => {
            let (vsock_stream, sockaddr) = vsock_listener.accept().map_err(Error::VsockAccept)?;

            if sockaddr.cid() != cid {
                Err(Error::IncorrectCid(sockaddr.cid()))
            } else {
                Ok(ForwarderSession::new(tcp_stream.into(), vsock_stream.into()))
            }
        }
        None => Err(Error::VsockAcceptTimeout),
    }
}

fn create_direct_forwarder_session(
    listener: &TcpListener,
    guest_address: IpAddr,
) -> Result<ForwarderSession> {
    let (local_stream, _) = listener.accept().map_err(Error::TcpAccept)?;
    let port = listener.local_addr().map_err(Error::TcpListenerPort)?.port();
    let remote_address = SocketAddr::new(guest_address, port);
    let remote_stream = TcpStream::connect_timeout(&remote_address, CHUNNEL_CONNECT_TIMEOUT)
        .map_err(Error::TcpConnect)?;
    Ok(ForwarderSession::new(local_stream.into(), remote_stream.into()))
}

// TODO(b/340126051): Host can receive opened ports from the guest.
fn run_forwarder_host(cid: i32, jni_env: JNIEnv, jni_cb: JObject) -> Result<()> {
    debug!("Starting forwarder_host");
    let mut sessions = ForwarderSessions::new_vsock(cid, jni_env, jni_cb);
    sessions.run()
}

fn run_direct_forwarder_host(guest_address: IpAddr) -> Result<()> {
    debug!("Starting direct forwarder_host for {guest_address}");
    let mut sessions = ForwarderSessions::new_direct(guest_address);
    sessions.run()
}

fn validate_forwarded_ports(ports: &[i32]) -> result::Result<Vec<u16>, String> {
    if ports.len() > MAX_FORWARDED_PORTS {
        return Err(format!("too many forwarded ports: {}", ports.len()));
    }
    ports
        .iter()
        .map(|port| {
            let port: u16 =
                (*port).try_into().map_err(|_| format!("invalid forwarded port: {port}"))?;
            if port < MIN_FORWARDED_PORT {
                return Err(format!("privileged forwarded port is not allowed: {port}"));
            }
            Ok(port)
        })
        .collect()
}

fn replace_listening_ports(
    env: &JNIEnv,
    ports: &JIntArray,
    update_queue: &Arc<Mutex<VecDeque<u16>>>,
    update_evt: &EventFd,
) -> result::Result<(), String> {
    let length =
        env.get_array_length(ports).map_err(|e| format!("failed to get port array length: {e}"))?;
    let mut buf = vec![0; length as usize];
    env.get_int_array_region(ports, 0, &mut buf)
        .map_err(|e| format!("failed to get port array: {e}"))?;
    let ports = validate_forwarded_ports(&buf)?;

    let mut queue = update_queue.lock().unwrap();
    queue.clear();
    queue.extend(ports);
    update_evt.write(1).map(|_| ()).map_err(|e| format!("failed to write update eventfd: {e}"))
}

fn run_vsock_from_jni(cid: jint, env: JNIEnv, callback: JObject) {
    // Clear shutdown event FD before running the legacy guest-agent forwarder.
    SHUTDOWN_EVT.write(1).expect("Failed to write shutdown event FD");
    SHUTDOWN_EVT.read().expect("Failed to consume shutdown event FD");

    match run_forwarder_host(cid, env, callback) {
        Ok(_) => info!("forwarder_host is terminated"),
        Err(e) => error!("Error on forwarder_host: {e:?}"),
    }
}

fn update_vsock_ports_from_jni(env: JNIEnv, ports: JIntArray) {
    if let Err(e) = replace_listening_ports(&env, &ports, &UPDATE_QUEUE, &UPDATE_EVT) {
        error!("Failed to update forwarder_host ports: {e}");
    }
}

/// JNI function for running the legacy guest-agent forwarder.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_runForwarderHost(
    env: JNIEnv,
    _class: JObject,
    cid: jint,
    callback: JObject,
) {
    run_vsock_from_jni(cid, env, callback);
}

/// Standalone koiTerminal package alias for the legacy guest-agent forwarder.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_koiterminal_DebianServiceImpl_runForwarderHost(
    env: JNIEnv,
    _class: JObject,
    cid: jint,
    callback: JObject,
) {
    run_vsock_from_jni(cid, env, callback);
}

/// JNI function for terminating the legacy guest-agent forwarder.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_terminateForwarderHost(
    _env: JNIEnv,
    _class: JObject,
) {
    SHUTDOWN_EVT.write(1).expect("Failed to write shutdown event FD");
}

/// Standalone koiTerminal package alias for terminating the guest-agent forwarder.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_koiterminal_DebianServiceImpl_terminateForwarderHost(
    _env: JNIEnv,
    _class: JObject,
) {
    SHUTDOWN_EVT.write(1).expect("Failed to write shutdown event FD");
}

/// JNI function for updating legacy guest-agent forwarding ports.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_terminal_DebianServiceImpl_updateListeningPorts(
    env: JNIEnv,
    _class: JObject,
    ports: JIntArray,
) {
    update_vsock_ports_from_jni(env, ports);
}

/// Standalone koiTerminal package alias for updating guest-agent forwarding ports.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_koiterminal_DebianServiceImpl_updateListeningPorts(
    env: JNIEnv,
    _class: JObject,
    ports: JIntArray,
) {
    update_vsock_ports_from_jni(env, ports);
}

/// Runs direct loopback-to-guest TCP forwarding without virtiofs or a guest agent.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_koiterminal_DirectForwarderHost_run(
    mut env: JNIEnv,
    _class: JObject,
    guest_address: JString,
    ports: JIntArray,
) {
    let guest_address: String = match env.get_string(&guest_address) {
        Ok(value) => value.into(),
        Err(e) => {
            error!("Failed to read direct forwarding guest address: {e}");
            return;
        }
    };
    let guest_address: IpAddr = match guest_address.parse() {
        Ok(value) => value,
        Err(e) => {
            error!("Invalid direct forwarding guest address: {e}");
            return;
        }
    };
    // A prior run can stop before it consumes its update event. Drain that stale signal before the
    // new port set is installed, or the old signal can remove the new listeners after startup.
    while DIRECT_UPDATE_EVT.read().is_ok() {}
    if let Err(e) = replace_listening_ports(&env, &ports, &DIRECT_UPDATE_QUEUE, &DIRECT_UPDATE_EVT)
    {
        error!("Failed to configure direct forwarding ports: {e}");
        return;
    }

    match run_direct_forwarder_host(guest_address) {
        Ok(_) => info!("direct forwarder_host is terminated"),
        Err(e) => error!("Error on direct forwarder_host: {e:?}"),
    }
}

/// Stops direct loopback-to-guest TCP forwarding.
#[no_mangle]
pub extern "C" fn Java_com_android_virtualization_koiterminal_DirectForwarderHost_shutdown(
    _env: JNIEnv,
    _class: JObject,
) {
    DIRECT_SHUTDOWN_EVT.write(1).expect("Failed to write direct shutdown event FD");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn direct_forwarder_connects_to_same_port_on_guest_address() {
        let host_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = host_listener.local_addr().unwrap().port();
        let guest_address = Ipv4Addr::new(127, 0, 0, 2);
        let guest_listener = TcpListener::bind((guest_address, port)).unwrap();
        let mut local_client = TcpStream::connect(host_listener.local_addr().unwrap()).unwrap();

        let mut session =
            create_direct_forwarder_session(&host_listener, guest_address.into()).unwrap();
        let (mut guest_server, _) = guest_listener.accept().unwrap();

        local_client.write_all(b"from-host").unwrap();
        assert!(!session.forward_from_local().unwrap());
        let mut from_host = [0_u8; 9];
        guest_server.read_exact(&mut from_host).unwrap();
        assert_eq!(&from_host, b"from-host");

        guest_server.write_all(b"from-guest").unwrap();
        assert!(!session.forward_from_remote().unwrap());
        let mut from_guest = [0_u8; 10];
        local_client.read_exact(&mut from_guest).unwrap();
        assert_eq!(&from_guest, b"from-guest");
    }

    #[test]
    fn forwarded_port_validation_is_bounded_and_unprivileged() {
        assert_eq!(validate_forwarded_ports(&[2222, 65535]).unwrap(), vec![2222, 65535]);
        assert!(validate_forwarded_ports(&[22]).unwrap_err().contains("privileged"));
        assert!(validate_forwarded_ports(&[-1]).unwrap_err().contains("invalid"));
        assert!(validate_forwarded_ports(&[2222; MAX_FORWARDED_PORTS + 1])
            .unwrap_err()
            .contains("too many"));
    }
}
