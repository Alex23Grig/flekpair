//! Finding trusted devices on the local network without Apple's help.
//!
//! macOS lists a device that has trusted the computer when both are on the same network.
//! Apple's service on Windows needs Bonjour for that, which comes with iTunes and not with the
//! driver alone, and on a PC it was tried on it listed nothing. So there the app asks the
//! network itself: one mDNS question about who offers Apple's device service.
//!
//! The question goes out from an ordinary port, so a device answers straight back to it. That
//! needs no port held open, and so no firewall rule. The answer names no device. It carries a
//! tag that only a computer holding the device's pairing record can work out, and that is how a
//! device is recognised.

use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use idevice::pairing_file::PairingFile;
use socket2::{Domain, Protocol, Socket, Type};
use tauri::{AppHandle, Manager};
use tokio::net::UdpSocket;
use tracing::debug;

use crate::{device::get_usbmuxd, error::chain};

/// Whether the app searches the network itself. macOS lists devices on Wi-Fi on its own, and a
/// search of ours there would only add a prompt for access to the local network.
pub const ENABLED: bool = cfg!(windows);

const SERVICE: &str = "_apple-mobdev2._tcp.local";
const MDNS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);

/// How long answers to one question are waited for.
const ANSWER_WAIT: Duration = Duration::from_millis(700);
const LOOK_EVERY: Duration = Duration::from_secs(3);
/// A device that hasn't answered for this long is gone. It spans a few missed questions.
const GONE_AFTER: Duration = Duration::from_secs(10);
/// How often pairing records are read again, in case a device was paired anew.
const RECORDS_EVERY: Duration = Duration::from_secs(30);

/// Where the UDIDs of devices seen on a cable are kept between runs, so that their records can
/// be asked for when they are next on the network instead.
const KNOWN_FILE: &str = "devices.txt";

const PTR: u16 = 12;
const TXT: u16 = 16;
const SRV: u16 = 33;
const A: u16 = 1;

/// What a device says about itself when asked.
#[derive(Debug, PartialEq)]
struct Announcement {
    address: Ipv4Addr,
    identifier: String,
    auth_tags: Vec<String>,
}

impl Announcement {
    /// Whether this is the device that `record` was made with.
    fn is_from(&self, record: &PairingFile) -> bool {
        let tags: Vec<&[u8]> = self.auth_tags.iter().map(String::as_bytes).collect();
        idevice::mdns::txt_record_matches(
            record.host_id.as_bytes(),
            self.identifier.as_bytes(),
            &tags,
        )
    }
}

/// Devices recognised on the network, and the pairing records that recognise them.
#[derive(Default)]
pub struct Nearby {
    /// UDIDs of devices this computer may hold a record for.
    known: Mutex<HashSet<String>>,
    records: Mutex<Records>,
    /// Where each recognised device last answered from, and when.
    seen: Mutex<HashMap<String, (Ipv4Addr, Instant)>>,
}

#[derive(Default)]
struct Records {
    by_udid: HashMap<String, PairingFile>,
    read: Option<Instant>,
}

impl Nearby {
    /// Notes devices that usbmuxd lists, whose records are worth asking for.
    pub fn learn<'a>(&self, udids: impl IntoIterator<Item = &'a str>) {
        let mut known = self.known.lock().unwrap();
        for udid in udids {
            if !known.contains(udid) {
                known.insert(udid.to_owned());
                // Read its record on the next round instead of when the others are due.
                self.records.lock().unwrap().read = None;
            }
        }
    }

    /// Devices that answered lately, with where from and the record each one trusts.
    pub fn found(&self) -> Vec<(String, Ipv4Addr, PairingFile)> {
        let records = self.records.lock().unwrap();
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (_, at))| at.elapsed() < GONE_AFTER)
            .filter_map(|(udid, (address, _))| {
                Some((udid.clone(), *address, records.by_udid.get(udid)?.clone()))
            })
            .collect()
    }

    /// Called when the device answers over a connection, which counts as much as an answer to
    /// a question: a device that is busy with a session can miss those.
    pub fn still_there(&self, udid: &str) {
        if let Some((_, at)) = self.seen.lock().unwrap().get_mut(udid) {
            *at = Instant::now();
        }
    }

    /// Reads the records again when they are due, asks the network, and notes who answered.
    pub(crate) async fn look(&self) {
        let due = self
            .records
            .lock()
            .unwrap()
            .read
            .is_none_or(|read| read.elapsed() >= RECORDS_EVERY);
        if due {
            self.read_records().await;
        }

        let announcements = ask_everywhere().await;
        let records = self.records.lock().unwrap();
        let mut seen = self.seen.lock().unwrap();
        for announcement in &announcements {
            let recognised = records
                .by_udid
                .iter()
                .find(|(_, record)| announcement.is_from(record));
            if let Some((udid, _)) = recognised {
                seen.insert(udid.clone(), (announcement.address, Instant::now()));
            }
        }
        seen.retain(|_, (_, at)| at.elapsed() < GONE_AFTER);
    }

    async fn read_records(&self) {
        let known: Vec<String> = self.known.lock().unwrap().iter().cloned().collect();
        let mut by_udid = HashMap::new();
        for udid in known {
            let Ok(mut usbmuxd) = get_usbmuxd().await else {
                // Without Apple's service there are no records to be had at all.
                return;
            };
            match usbmuxd.get_pair_record(&udid).await {
                Ok(record) => {
                    by_udid.insert(udid, record);
                }
                // Seen on a cable, but never trusted.
                Err(e) => debug!("No pairing record for {udid}: {}", chain(&e)),
            }
        }
        *self.records.lock().unwrap() = Records {
            by_udid,
            read: Some(Instant::now()),
        };
    }
}

/// Keeps looking for devices on the network for as long as the app runs.
pub fn watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let nearby = app.state::<Nearby>();
        let file = app
            .path()
            .app_data_dir()
            .ok()
            .map(|dir| dir.join(KNOWN_FILE));
        let remembered = file.as_deref().map(remembered).unwrap_or_default();
        nearby.learn(remembered.iter().map(String::as_str));
        nearby.learn(paired_with_this_computer().iter().map(String::as_str));

        let mut written = remembered.len();
        loop {
            nearby.look().await;

            let known: Vec<String> = nearby.known.lock().unwrap().iter().cloned().collect();
            if let Some(file) = file.as_deref().filter(|_| known.len() != written) {
                match remember(file, &known) {
                    Ok(()) => written = known.len(),
                    Err(e) => debug!("Couldn't note the known devices: {e}"),
                }
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    });
}

fn remembered(file: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn remember(file: &std::path::Path, udids: &[String]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, udids.join("\n"))
}

/// The UDIDs Apple's service on Windows holds records for: it names each record's file after
/// its device. Reading the folder is a bonus that covers devices paired before this app was
/// installed, so failing to is no error.
fn paired_with_this_computer() -> Vec<String> {
    let Some(folder) = std::env::var_os("ProgramData")
        .filter(|_| cfg!(windows))
        .map(|data| PathBuf::from(data).join("Apple").join("Lockdown"))
    else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };

    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "plist")
        })
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        // The one file there that isn't a device's.
        .filter(|name| name != "SystemConfiguration")
        .collect()
}

/// Asks on every network the computer is on. A VPN's own network is one of them, so the
/// question has to go out on each rather than wherever the system would send it.
async fn ask_everywhere() -> Vec<Announcement> {
    let locals = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match interface.ip() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        });

    let mut found = Vec::new();
    for (local, asked) in
        futures::future::join_all(locals.map(|local| async move { (local, ask(local).await) }))
            .await
    {
        match asked {
            Ok(announcements) => {
                for announcement in announcements {
                    if !found.contains(&announcement) {
                        found.push(announcement);
                    }
                }
            }
            Err(e) => debug!("Couldn't ask from {local}: {e}"),
        }
    }
    found
}

/// Asks on the network that `local` is this computer's address on.
async fn ask(local: Ipv4Addr) -> std::io::Result<Vec<Announcement>> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // Any port but mDNS's own, see the top of the file.
    socket.bind(&SocketAddrV4::new(local, 0).into())?;
    socket.set_multicast_if_v4(&local)?;
    socket.set_multicast_ttl_v4(255)?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket.into())?;
    socket.send_to(&question(), MDNS).await?;

    let mut found = Vec::new();
    let mut reply = [0u8; 9000];
    let until = tokio::time::Instant::now() + ANSWER_WAIT;
    while let Ok(Ok((length, from))) =
        tokio::time::timeout_at(until, socket.recv_from(&mut reply)).await
    {
        if let SocketAddr::V4(from) = from {
            found.extend(announcements(&reply[..length], *from.ip()));
        }
    }
    Ok(found)
}

/// The question: who offers Apple's device service?
fn question() -> Vec<u8> {
    // The header: no id, no flags, one question.
    let mut packet = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in SERVICE.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    // The name ends, then: a PTR record, in the internet class.
    packet.extend_from_slice(&[0, 0, PTR as u8, 0, 1]);
    packet
}

struct Record<'a> {
    name: String,
    kind: u16,
    /// Where `data` starts in the packet, for the names in it that point elsewhere.
    start: usize,
    data: &'a [u8],
}

/// The devices described in an answer that came `from` an address.
fn announcements(packet: &[u8], from: Ipv4Addr) -> Vec<Announcement> {
    let Some(records) = records(packet) else {
        return Vec::new();
    };
    let named = |kind: u16, name: &str| {
        records
            .iter()
            .find(move |record| record.kind == kind && record.name.eq_ignore_ascii_case(name))
    };

    records
        .iter()
        .filter(|record| record.kind == TXT)
        .filter(|record| record.name.to_ascii_lowercase().ends_with(SERVICE))
        .filter_map(|text| {
            let mut identifier = None;
            let mut auth_tags = Vec::new();
            for entry in strings(text.data) {
                match entry.split_once('=') {
                    Some(("identifier", value)) => identifier = Some(value.to_owned()),
                    // One per computer the device trusts: authTag, authTag#1 and so on.
                    Some((key, value)) if key == "authTag" || key.starts_with("authTag#") => {
                        auth_tags.push(value.to_owned())
                    }
                    _ => {}
                }
            }

            // The address the device gives for itself, or else where its answer came from.
            let address = named(SRV, &text.name)
                .and_then(|service| name(packet, service.start + 6))
                .and_then(|(host, _)| named(A, &host))
                .and_then(|address| <[u8; 4]>::try_from(address.data).ok())
                .map_or(from, Ipv4Addr::from);

            Some(Announcement {
                address,
                identifier: identifier?,
                auth_tags,
            })
        })
        .collect()
}

/// Every record in a DNS packet, or `None` if it isn't a well-formed one.
fn records(packet: &[u8]) -> Option<Vec<Record<'_>>> {
    let number = |at: usize| Some(u16::from_be_bytes([*packet.get(at)?, *packet.get(at + 1)?]));
    let questions = number(4)?;
    let answers = number(6)? as usize + number(8)? as usize + number(10)? as usize;

    let mut at = 12;
    for _ in 0..questions {
        // A question is a name, a type and a class.
        at = name(packet, at)?.1 + 4;
    }

    let mut records = Vec::new();
    for _ in 0..answers {
        // A record is a name, a type, a class, how long it holds, and its data with a length.
        let (owner, next) = name(packet, at)?;
        let kind = number(next)?;
        let start = next + 10;
        let end = start + number(next + 8)? as usize;
        records.push(Record {
            name: owner,
            kind,
            start,
            data: packet.get(start..end)?,
        });
        at = end;
    }
    Some(records)
}

/// The name at a place in a packet, and where what follows it starts. A name may continue
/// somewhere earlier in the packet, to save repeating itself.
fn name(packet: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut after = None;
    // No well-formed name takes more steps than the packet has bytes; one that points round in
    // circles would.
    for _ in 0..packet.len() {
        let length = *packet.get(at)? as usize;
        if length == 0 {
            return Some((labels.join("."), after.unwrap_or(at + 1)));
        }
        if length & 0xC0 == 0xC0 {
            after.get_or_insert(at + 2);
            at = (length & 0x3F) << 8 | *packet.get(at + 1)? as usize;
        } else if length < 64 {
            let label = packet.get(at + 1..at + 1 + length)?;
            labels.push(String::from_utf8_lossy(label).into_owned());
            at += 1 + length;
        } else {
            return None;
        }
    }
    None
}

/// The strings of a TXT record, each of which says its own length first.
fn strings(mut data: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    while let Some((&length, rest)) = data.split_first() {
        let Some(string) = rest.get(..length as usize) else {
            break;
        };
        strings.push(String::from_utf8_lossy(string).into_owned());
        data = &rest[length as usize..];
    }
    strings
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::STANDARD};

    use super::*;

    const INSTANCE: &str = "aa:bb:cc:dd:ee:ff@fe80::1-supportsRP-26";

    fn labels(name: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        for label in name.split('.') {
            bytes.push(label.len() as u8);
            bytes.extend_from_slice(label.as_bytes());
        }
        bytes
    }

    fn record(owner: &[u8], kind: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = owner.to_vec();
        bytes.extend_from_slice(&[0, kind as u8, 0, 1, 0, 0, 0, 10]);
        bytes.extend_from_slice(&(data.len() as u16).to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    /// An answer laid out the way an iPhone's is: the service's name once, at byte 12, and
    /// pointed back to from then on.
    fn answer(entries: &[&str], address: Option<[u8; 4]>) -> Vec<u8> {
        let service = [0xC0, 12];
        let mut packet = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3];
        if address.is_none() {
            packet[11] = 2;
        }

        // PTR: the service, and the instance that offers it.
        let instance_at = packet.len() + labels(SERVICE).len() + 1 + 10;
        let mut instance = labels(INSTANCE);
        instance.extend_from_slice(&service);
        packet.extend(record(&[labels(SERVICE), vec![0]].concat(), PTR, &instance));
        let instance = [0xC0, instance_at as u8];

        // SRV: priority, weight and port, then the host.
        let host_at = packet.len() + 2 + 10 + 6;
        let host = [labels("iPhone"), labels("local"), vec![0]].concat();
        packet.extend(record(
            &instance,
            SRV,
            &[&[0, 0, 0, 0, 0x7E, 0xF2], &host[..]].concat(),
        ));

        let mut text = Vec::new();
        for entry in entries {
            text.push(entry.len() as u8);
            text.extend_from_slice(entry.as_bytes());
        }
        packet.extend(record(&instance, TXT, &text));

        if let Some(address) = address {
            packet.extend(record(&[0xC0, host_at as u8], A, &address));
        }
        packet
    }

    #[test]
    fn reads_a_devices_answer() {
        let from = Ipv4Addr::new(192, 168, 0, 9);
        let entries = [
            "identifier=ABC-123",
            "authTag=AAAA",
            "authTag#1=BBBB",
            "other=x",
        ];

        assert_eq!(
            announcements(&answer(&entries, Some([192, 168, 0, 20])), from),
            [Announcement {
                address: Ipv4Addr::new(192, 168, 0, 20),
                identifier: "ABC-123".into(),
                auth_tags: vec!["AAAA".into(), "BBBB".into()],
            }]
        );
        // Without an address of its own, the device is where its answer came from.
        assert_eq!(
            announcements(&answer(&entries, None), from)[0].address,
            from
        );
        // Nothing to go on without the identifier the tags are worked out from.
        assert_eq!(announcements(&answer(&["authTag=AAAA"], None), from), []);
    }

    #[test]
    fn makes_nothing_of_a_broken_answer() {
        let from = Ipv4Addr::LOCALHOST;
        let whole = answer(&["identifier=ABC-123", "authTag=AAAA"], Some([10, 0, 0, 2]));

        for length in 0..whole.len() {
            assert_eq!(announcements(&whole[..length], from), [], "cut at {length}");
        }
        // A name that points at itself.
        let circular = [&[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0][..], &[0xC0, 12]].concat();
        assert_eq!(announcements(&circular, from), []);
    }

    #[test]
    fn recognises_a_device_by_its_pairing_record_alone() {
        let tag = |host_id: &str| {
            STANDARD.encode(idevice::mdns::derive_auth_tag(
                host_id.as_bytes(),
                b"ABC-123",
            ))
        };
        let trusting = |tags: Vec<String>| {
            idevice::mdns::txt_record_matches(
                b"THIS-COMPUTER",
                b"ABC-123",
                &tags.iter().map(String::as_bytes).collect::<Vec<_>>(),
            )
        };

        assert!(trusting(vec![
            tag("ANOTHER-COMPUTER"),
            tag("THIS-COMPUTER")
        ]));
        assert!(!trusting(vec![tag("ANOTHER-COMPUTER")]));
        assert!(!trusting(vec![]));
    }
}
