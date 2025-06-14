use {
    aargvark::{
        vark,
        Aargvark,
    },
    futures::{
        future::join_all,
        TryStreamExt,
    },
    loga::{
        ea,
        fatal,
        DebugDisplay,
        Log,
        ResultContext,
    },
    moka::future::{
        Cache,
        CacheBuilder,
    },
    std::{
        num::NonZero,
        time::Duration,
    },
    tokio::task::yield_now,
    tokio_util::sync::CancellationToken,
    zbus::{
        names::{
            InterfaceName,
            UniqueName,
        },
        Connection,
        MessageStream,
    },
};

#[derive(Clone)]
struct ReplyTypeRemote {
    destination: Option<UniqueName<'static>>,
    serial: NonZero<u32>,
}

#[derive(Clone)]
enum ReplyType {
    //. TODO subscribe to name changes, stop when evicted
    //. LocalNameChanged,
    Remote(ReplyTypeRemote),
}

type ReplyCache = Cache<NonZero<u32>, ReplyType>;
type NameCache = Cache<InterfaceName<'static>, UniqueName<'static>>;

async fn side(
    log: Log,
    die: CancellationToken,
    forward: ReplyCache,
    reverse: ReplyCache,
    mut source_rx: MessageStream,
    dest_tx: Connection,
    dest_names: NameCache,
    busname: String
) -> Option<Result<(), loga::Error>> {
    let _die = die.clone().drop_guard();
    return die.run_until_cancelled_owned(async move {
        loop {
            let Some(msg) = source_rx.try_next().await.context("Error receiving message from source")? else {
                break;
            };
            log.log_with(
                loga::DEBUG,
                "Got message",
                ea!(msg = msg.dbg_str(), body = String::from_utf8_lossy(&msg.data())),
            );
            let header = msg.header();
            if header.member().map(|x| x.as_str()) == Some("NameAcquired") {
                // https://github.com/dbus2/zbus/issues/1318
                continue;
            }
            let mut out = zbus::message::Builder::from(header.clone());
            if let Some(reply_serial) = header.reply_serial() {
                let Some(reply_info) = forward.remove(&reply_serial).await else {
                    log.log_with(
                        loga::WARN,
                        format!("Couldn't find destination request for reply to [{}]", reply_serial),
                        ea!(msg = msg.dbg_str(), body = String::from_utf8_lossy(&msg.data())),
                    );
                    continue;
                };
                match reply_info {
                    //. ReplyType::LocalNameChanged => {
                    //.     continue;
                    //. },
                    ReplyType::Remote(remote_info) => {
                        if let Some(destination) = remote_info.destination {
                            out = out.destination(destination).unwrap();
                        }
                        out = out.reply_serial(Some(remote_info.serial));
                    },
                }
            } else {
                if header.destination().is_some() {
                    if let Some(iface) = header.interface() {
                        let dest_name = dest_names.try_get_with(iface.to_owned(), async {
                            yield_now().await;
                            let resp =
                                dest_tx
                                    .call_method(
                                        Some("org.freedesktop.DBus"),
                                        "/org/freedesktop/DBus",
                                        Some("org.freedesktop.DBus"),
                                        "GetNameOwner",
                                        &busname,
                                    )
                                    .await
                                    .context_with("Error looking up destination name", ea!(name = iface))?;
                            return Ok(resp.body().deserialize::<UniqueName<'_>>()?.to_owned()) as
                                Result<_, loga::Error>;
                        }).await.map_err(loga::err)?;
                        out = out.destination(dest_name).unwrap();
                    }
                }
            };

            // # Forward...
            let new_serial = zbus::message::PrimaryHeader::new(zbus::message::Type::MethodCall, 100).serial_num();

            // Add reply lookup - generated reply serial to original sender + reply serial
            reverse.insert(new_serial, ReplyType::Remote(ReplyTypeRemote {
                destination: header.sender().as_ref().map(|x| (*x).to_owned()),
                serial: header.primary().serial_num(),
            })).await;

            // Replace the serial
            out = out.serial(new_serial);
            if let Some(x) = dest_tx.unique_name() {
                out = out.sender(x).unwrap();
            }

            // Build the message and send
            let sig = header.signature().clone();
            let msg = unsafe {
                out.build_raw_body(&msg.body().data(), sig, msg.data().fds().iter().map(|x| match x {
                    zbus::zvariant::Fd::Borrowed(_borrowed_fd) => panic!(),
                    zbus::zvariant::Fd::Owned(owned_fd) => zbus::zvariant::OwnedFd::from(
                        owned_fd.try_clone().unwrap(),
                    ),
                }).collect()).unwrap()
            };
            log.log_with(
                loga::DEBUG,
                "Forwarding translated message",
                ea!(msg = msg.dbg_str(), body = String::from_utf8_lossy(&msg.data())),
            );
            dest_tx.send(&msg).await.context("Error forwarding upstream message to destination")?;
        }
        return Ok(()) as Result<(), loga::Error>;
    }).await;
}

/// Connects to 2 dbus sockets and registers as a server for a set of services on
/// the downstream, and acts as a client on the upstream.  Any requests it receives
/// as a server it'll forward to the cooresponding server on the upstream side.
#[derive(Aargvark)]
struct Args {
    /// The connection with the desired servers. As you'd see in
    /// `DBUS_SESSION_BUS_ADDRESS`, ex: `unix:path=/run/user/1910/bus`
    #[vark(flag = "--upstream")]
    upstream: String,
    /// The connection with no servers, where this will pose as a server. Ex:
    /// `unix:path=/run/user/1910/bus`
    #[vark(flag = "--downstream")]
    downstream: String,
    /// The names to act as a server for/forward. Ex: `org.freedesktop.Notifications`
    #[vark(flag = "--register")]
    register: Vec<String>,
    /// Enable verbose logging
    debug: Option<()>,
}

async fn main1() -> Result<(), loga::Error> {
    let args = vark::<Args>();
    let log = Log::new_root(if args.debug.is_some() {
        loga::DEBUG
    } else {
        loga::INFO
    });
    let downstream =
        zbus::connection::Builder::address(args.downstream.as_str())
            .context_with("Invalid downstream addr", ea!(addr = args.downstream))?
            .build()
            .await
            .context_with("Error opening downstream connection", ea!(addr = args.downstream))?;
    let upstream =
        zbus::connection::Builder::address(args.upstream.as_str())
            .context_with("Invalid upstream addr", ea!(addr = args.upstream))?
            .build()
            .await
            .context_with("Error opening upstream connection", ea!(addr = args.upstream))?;
    let die = CancellationToken::new();
    let mut tasks = vec![];
    for name in &args.register {
        let downup_serials: ReplyCache = CacheBuilder::default().time_to_idle(Duration::from_secs(60 * 10)).build();
        let updown_serials: ReplyCache = CacheBuilder::default().time_to_idle(Duration::from_secs(60 * 10)).build();
        let upstream_names = CacheBuilder::default().time_to_idle(Duration::from_secs(5)).build();
        let downstream_names = CacheBuilder::default().time_to_idle(Duration::from_secs(5)).build();
        downstream
            .request_name(name.as_str())
            .await
            .context(format!("Error registering name [{}] on downstream", name))?;
        tasks.push(
            side(
                log.fork(ea!(name = name, side = "up->down")),
                die.clone(),
                updown_serials.clone(),
                downup_serials.clone(),
                MessageStream::from(&upstream),
                downstream.clone(),
                downstream_names.clone(),
                name.clone()
            ),
        );
        tasks.push(
            side(
                log.fork(ea!(name = name, side = "down->up")),
                die.clone(),
                downup_serials.clone(),
                updown_serials.clone(),
                MessageStream::from(&downstream),
                upstream.clone(),
                upstream_names.clone(),
                name.clone()
            ),
        );
    }
    let mut errors = vec![];
    for res in join_all(tasks).await {
        let Some(res) = res else {
            continue;
        };
        if let Err(e) = res {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        return Err(loga::agg_err("Task failures", errors));
    }
    return Ok(());
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    match main1().await {
        Ok(_) => { },
        Err(e) => fatal(e),
    }
}
