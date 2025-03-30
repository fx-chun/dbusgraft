use {
    aargvark::{
        vark,
        Aargvark,
    },
    dbus::{
        channel::{
            Channel,
            MatchingReceiver,
            Sender,
        },
        message::MatchRule,
        nonblock::SyncConnection,
    },
    dbus_tokio::connection::from_channel,
    loga::{
        conversion::ResultIgnore,
        ea,
        fatal,
        ErrContext,
        ResultContext,
    },
    tokio::{
        join,
        select,
        signal::unix::SignalKind,
        spawn,
    },
};

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
}

async fn main1() -> Result<(), loga::Error> {
    let args = vark::<Args>();
    let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt()).context("Error hooking into SIGINT")?;
    let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate()).context("Error hooking into SIGTERM")?;
    let (downstream_work, downstream) =
        from_channel::<SyncConnection>(
            Channel::open_private(
                &args.downstream,
            ).context_with("Error opening downstream connection", ea!(addr = args.downstream))?,
        ).context("Error initializing dbus downstream connection")?;
    let downstream_work = spawn(async {
        downstream_work.await
    });
    let (upstream_work, upstream) =
        from_channel::<SyncConnection>(
            Channel::open_private(
                &args.upstream,
            ).context_with("Error opening upstream connection", ea!(addr = args.upstream))?,
        ).context("Error initializing dbus upstream connection")?;
    let upstream_work = spawn(async {
        upstream_work.await
    });
    downstream.start_receive(MatchRule::new_method_call(), Box::new({
        let upstream = upstream.clone();
        move |msg, _conn| {
            // Failure = worker has already exited (shutting down)
            upstream.send(msg).ignore();
            return true;
        }
    }));
    upstream.start_receive(MatchRule::default(), Box::new({
        let downstream = downstream.clone();
        move |msg, _conn| {
            downstream.send(msg).ignore();
            return true;
        }
    }));
    let mut errors = vec![];
    let upstream_abort = upstream_work.abort_handle();
    let downstream_abort = downstream_work.abort_handle();
    let (
        //. .
        upstream,
        downstream,
        _,
    ) = join!(
        //. .
        upstream_work,
        downstream_work,
        async {
            select!{
                r = async {
                    for name in args.register {
                        downstream
                            .request_name(&name, false, true, false)
                            .await
                            .context(format!("Error registering name [{}]", name))?;
                    }
                    return Ok(());
                }
                => {
                    if let Err(e) = r {
                        errors.push(e);
                    }
                },
                _ = sigint.recv() => {
                },
                _ = sigterm.recv() => {
                },
            }
            upstream_abort.abort();
            downstream_abort.abort();
            return ();
        }
    );
    if let Err(e) = upstream {
        if !e.is_cancelled() {
            errors.push(e.context("Upstream worker failed"));
        }
    }
    if let Err(e) = downstream {
        if !e.is_cancelled() {
            errors.push(e.context("Downstream worker failed"));
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
