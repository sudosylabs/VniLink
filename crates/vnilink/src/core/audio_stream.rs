//! Independent Audio pipes on a neutral parent. Mute is receiver-local.
// trace: records every ignored failure
use super::{
    service_stream::{preamble, read, status, write, HostedEnd, HostedEnding},
    *,
};
use crate::{audio::*, CloseCause, ServiceKind, ServiceLease, ServicePhase};
use iroh::endpoint::{RecvStream, SendStream};
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct RemoteAudioService {
    config: AudioConfig,
    lease: ServiceLease,
    stop: tokio::sync::watch::Sender<bool>,
    inbox: Arc<AudioSource>,
}
impl RemoteAudioService {
    pub fn config(&self) -> AudioConfig {
        self.config.clone()
    }
    pub fn info(&self) -> &crate::ServiceInfo {
        self.lease.info()
    }
    pub fn is_cancelled(&self) -> bool {
        self.lease.is_cancelled()
    }
    pub fn try_recv(&self) -> Option<AudioPacket> {
        self.inbox.pop()
    }
    pub fn close(&self) {
        self.stop.send_replace(true);
        self.inbox.close();
    }
}
impl Drop for RemoteAudioService {
    fn drop(&mut self) {
        self.close();
    }
}

impl Core {
    pub fn set_audio_factory(
        &self,
        factory: Option<Arc<dyn HostAudioFactory>>,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        *lock(&self.shared.audio_factory) = factory;
        for record in state.hosted_services.values_mut().filter(|r| {
            matches!(
                r.info.kind,
                ServiceKind::SystemAudio | ServiceKind::Microphone
            )
        }) {
            if record.change(
                record.info.generation,
                ServicePhase::Closed(CloseCause::DeviceLoss),
            ) {
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
        Ok(())
    }
    pub fn remote_audio_devices(
        &self,
        connection: u64,
        generation: u64,
        kind: AudioKind,
    ) -> Result<Vec<AudioDevice>, Error> {
        let _engine = self.engine();
        let (link, stamp) = self.service_link(connection, generation)?;
        let mut stop = self
            .alive_state()?
            .outgoing
            .get(&connection)
            .ok_or(Error::NoSession)?
            .stop
            .subscribe();
        self.runtime.block_on(async {
            let operation=tokio::time::timeout(TIMEOUT,async {
                let(mut send,mut recv)=link.open_bi().await.map_err(Error::network_debug)?;
                write(&mut send,preamble(stamp,0,2,kind.wire())).await?;
                status(&read(&mut recv).await?)?;
                let devices:Vec<AudioDevice>=serde_json::from_slice(&read(&mut recv).await?).map_err(|_|Error::Malformed)?;
                validate_devices(&devices)?;
                if devices.iter().any(|d|d.kind!=kind){return Err(Error::Malformed);}
                let _=send.finish();Ok(devices)
            });
            if *stop.borrow(){return Err(Error::NoSession);}
            tokio::select!{biased;_=stop.changed()=>Err(Error::NoSession),result=operation=>result.map_err(|_|Error::Timeout)?}
        })
    }
    pub fn open_audio_service(
        &self,
        id: u64,
        generation: u64,
        config: AudioConfig,
    ) -> Result<RemoteAudioService, Error> {
        let _engine = self.engine();
        config.validate()?;
        let mut lease = self.service_lease(id, generation)?;
        if lease.info().kind != config.kind.service() {
            return Err(Error::InvalidSetting(
                "audio source kind differs from owner",
            ));
        }
        let handle = lease.clone();
        let (link, stamp) = self.service_link(lease.info().connection_id, generation)?;
        let stop = {
            let mut state = self.alive_state()?;
            let record = state.services.get_mut(&id).ok_or(Error::NoSession)?;
            if record.stream_open {
                return Err(Error::Busy);
            }
            if lease.is_cancelled() {
                return Err(Error::NoSession);
            }
            record.stream_open = true;
            record.stop.clone()
        };
        let inbox = AudioSource::new();
        let packets = inbox.clone();
        let shared = self.shared.clone();
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        self.runtime.spawn(async move {
            let opened = lease
                .run(async {
                    tokio::time::timeout(TIMEOUT, async {
                        let (mut send, mut recv) =
                            link.open_bi().await.map_err(Error::network_debug)?;
                        write(&mut send, preamble(stamp, id, 1, config.kind.wire())).await?;
                        write(
                            &mut send,
                            serde_json::to_vec(&config).map_err(|_| Error::Malformed)?,
                        )
                        .await?;
                        status(&read(&mut recv).await?)?;
                        let actual: AudioConfig = serde_json::from_slice(&read(&mut recv).await?)
                            .map_err(|_| Error::Malformed)?;
                        actual.validate()?;
                        if actual != config {
                            return Err(Error::Malformed);
                        }
                        Ok((send, recv, actual))
                    })
                    .await
                    .map_err(|_| Error::Timeout)?
                })
                .await;
            let mut phase = ServicePhase::Closed(CloseCause::PeerClose);
            match opened {
                Err(error) => {
                    phase = ServicePhase::Failed(error.to_string());
                    crate::ignored!(
                        crate::trace::AUDIO,
                        "the caller stopped waiting for its answer",
                        ready.send(Err(error))
                    );
                }
                Ok((mut send, mut recv, actual)) => {
                    let format = actual.format().unwrap();
                    let kind = actual.kind;
                    crate::ignored!(
                        crate::trace::AUDIO,
                        "the caller stopped waiting for its answer",
                        ready.send(Ok(actual))
                    );
                    let run = async {
                        loop {
                            let bytes = tokio::time::timeout(
                                Duration::from_secs(3),
                                read_packet(&mut recv),
                            )
                            .await
                            .map_err(|_| Error::Timeout)??;
                            if bytes == [0] {
                                return Ok("stopped");
                            }
                            if bytes == [4] {
                                return Ok("withdrawn");
                            }
                            if bytes == [3] && kind == AudioKind::System {
                                continue; // Source is silent; this is liveness, not an Opus frame.
                            }
                            if bytes == [2] {
                                return Err(Error::Unsupported("remote audio capture ended"));
                            }
                            let packet =
                                AudioPacket::decode(&bytes).map_err(|_| Error::Malformed)?;
                            if packet.format != format {
                                return Err(Error::Malformed);
                            }
                            packets.publish(packet)?;
                        }
                    };
                    let ended = lease.run(run).await;
                    // A viewer that closed the audio itself has no other reason
                    // to give; otherwise the host's word, or what failed.
                    let closed_here =
                        lease.is_cancelled() && super::services::closed_here(&shared, id, generation);
                    tracing::info!(
                        target: crate::trace::AUDIO,
                        service = id,
                        kind = ?kind,
                        ended = (!closed_here).then(|| match &ended {
                            Ok(words) => *words,
                            Err(Error::Unsupported("remote audio capture ended")) => "capture ended",
                            Err(error) => error.name(),
                        }),
                        closed_here,
                        dropped = packets.dropped(),
                        "audio ended"
                    );
                    if ended.is_err() && !lease.is_cancelled() {
                        phase = ServicePhase::Failed("audio pipe ended".into());
                    } else if lease.is_cancelled() {
                        phase = ServicePhase::Closed(CloseCause::OwnerClose);
                    }
                    crate::ignored!(
                        crate::trace::AUDIO,
                        "a stream that cannot finish is closed with its link",
                        send.finish()
                    );
                }
            }
            packets.close();
            let mut state = lock(&shared.state);
            if let Some(record) = state
                .services
                .get_mut(&id)
                .filter(|r| r.info.generation == generation)
            {
                if record.change(generation, phase) {
                    shared.events.publish(CoreEvent::ServiceChanged {
                        service: record.info.clone(),
                    });
                }
            }
        });
        let config = result
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::Timeout)??;
        Ok(RemoteAudioService {
            config,
            lease: handle,
            stop,
            inbox,
        })
    }
}

fn validate_devices(devices: &[AudioDevice]) -> Result<(), Error> {
    let mut ids = std::collections::HashSet::new();
    if devices.len() > 64
        || devices.iter().any(|d| {
            d.id.is_empty()
                || d.id.len() > 512
                || d.name.len() > 1024
                || !ids.insert((&d.id, d.kind.wire()))
        })
    {
        return Err(Error::Malformed);
    }
    Ok(())
}
async fn response(send: &mut SendStream, code: u8) -> Result<(), Error> {
    write(send, vec![1, code, 0]).await
}
pub(super) async fn list(
    shared: Arc<Shared>,
    peer: [u8; 32],
    kind: AudioKind,
    mut send: SendStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    if !lock(&shared.state)
        .trust
        .service_granted(&peer, kind.service())
    {
        response(&mut send, 6).await?;
        return Ok(());
    }
    let factory = lock(&shared.audio_factory).clone();
    let Some(factory) = factory.filter(|f| f.supports(kind)) else {
        response(&mut send, 1).await?;
        return Ok(());
    };
    let run = async {
        let devices = tokio::task::spawn_blocking(move || factory.devices())
            .await
            .map_err(|_| Error::Unsupported("audio enumeration"))??;
        validate_devices(&devices)?;
        let devices: Vec<_> = devices.into_iter().filter(|d| d.kind == kind).collect();
        if !lock(&shared.state)
            .trust
            .service_granted(&peer, kind.service())
        {
            response(&mut send, 6).await?;
            return Ok(());
        }
        response(&mut send, 0).await?;
        write(
            &mut send,
            serde_json::to_vec(&devices).map_err(|_| Error::Malformed)?,
        )
        .await?;
        crate::ignored!(
            crate::trace::AUDIO,
            "a stream that cannot finish is closed with its link",
            send.finish()
        );
        Ok(())
    };
    if *parent.borrow() {
        return Err(Error::NoSession);
    }
    tokio::select! {biased;_=parent.changed()=>Err(Error::NoSession),result=tokio::time::timeout(TIMEOUT,run)=>result.map_err(|_|Error::Timeout)?}
}
async fn read_packet(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    crate::sidestream::read_frame(
        recv,
        |len| {
            if len == 0 || len > 1217 {
                Err(Error::Malformed)
            } else {
                Ok(())
            }
        },
        |bytes| Ok(bytes.to_vec()),
    )
    .await
}
pub(super) async fn serve(
    shared: Arc<Shared>,
    id: u64,
    kind: AudioKind,
    mut send: SendStream,
    mut recv: RecvStream,
    mut parent: tokio::sync::watch::Receiver<bool>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<(), Error> {
    let factory = lock(&shared.audio_factory)
        .clone()
        .filter(|f| f.supports(kind))
        .ok_or(Error::Unsupported("audio backend"))?;
    let startup_parent = parent.clone();
    let startup_stop = stop.clone();
    // Says how the stream ended when it ends, however it ends: a task the
    // engine drops while tearing down records it too.
    let ending = HostedEnding::new(&shared, id, move |ended| {
        tracing::debug!(
            target: crate::trace::AUDIO,
            service = id,
            kind = ?kind,
            ended,
            "audio stream ended"
        );
    });
    let run = async {
        let opening = async {
            let config: AudioConfig =
                serde_json::from_slice(&read(&mut recv).await?).map_err(|_| Error::Malformed)?;
            config.validate()?;
            if config.kind != kind {
                return Err(Error::Malformed);
            }
            let requested = config.clone();
            let startup = crate::BackendStartup::scoped(
                startup_parent.clone(),
                startup_stop.clone(),
                TIMEOUT,
            );
            let _guard = startup.guard();
            let open = tokio::task::spawn_blocking(move || {
                startup.check()?;
                let devices = factory.devices()?;
                validate_devices(&devices)?;
                if !devices
                    .iter()
                    .any(|d| d.id == config.device_id && d.kind == kind)
                {
                    return Err(Error::Unsupported("approved audio source"));
                }
                startup.check()?;
                let backend = factory.open(config, startup.clone())?;
                startup.check()?;
                Ok::<_, Error>(backend)
            });
            let backend = tokio::select! {
                biased;
                _ = read(&mut recv) => {
                    ending.why(HostedEnd::ViewerClosed);
                    return Err(Error::NoSession);
                }
                result = open => result.map_err(|_| Error::Unsupported("audio startup"))??,
            };
            if *startup_parent.borrow() || *startup_stop.borrow() {
                return Err(Error::NoSession);
            }
            backend.config.validate()?;
            if backend.config != requested {
                return Err(Error::Malformed);
            }
            Ok(backend)
        };
        let opened = tokio::time::timeout(TIMEOUT, opening).await;
        if !matches!(opened, Ok(Ok(_))) {
            tracing::info!(
                target: crate::trace::AUDIO,
                service = id,
                kind = ?kind,
                error = match &opened {
                    Ok(Err(error)) => error.name(),
                    _ => "timeout",
                },
                reason = match &opened {
                    Ok(Err(error)) => error.reason(),
                    _ => None,
                },
                "audio not opened"
            );
        }
        let backend = match opened {
            Ok(Ok(backend)) => backend,
            _ => {
                response(&mut send, 1).await?;
                return Err(Error::Unsupported("audio startup"));
            }
        };
        // Original owner/parent still live before admitting native capture.
        if *startup_parent.borrow() || *startup_stop.borrow() {
            return Err(Error::NoSession);
        }
        if !lock(&shared.state)
            .hosted_services
            .get(&id)
            .is_some_and(|r| r.info.kind == kind.service())
        {
            return Err(Error::NoSession);
        }
        response(&mut send, 0).await?;
        write(
            &mut send,
            serde_json::to_vec(&backend.config).map_err(|_| Error::Malformed)?,
        )
        .await?;
        tracing::info!(
            target: crate::trace::AUDIO,
            service = id,
            kind = ?kind,
            channels = backend.config.channels,
            voice = backend.config.voice,
            "audio opened"
        );
        let sending = async {
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_write = std::time::Instant::now();
            loop {
                if backend.source.is_cancelled() {
                    tracing::info!(
                        target: crate::trace::AUDIO,
                        service = id,
                        failed = backend.source.has_failed(),
                        dropped = backend.source.dropped(),
                        "audio capture ended"
                    );
                    ending.why(if backend.source.has_failed() {
                        HostedEnd::Failed("capture failed")
                    } else {
                        HostedEnd::Finished("capture ended")
                    });
                    write(
                        &mut send,
                        vec![if backend.source.has_failed() { 2 } else { 0 }],
                    )
                    .await?;
                    return Ok(());
                }
                if let Some(packet) = backend.source.pop() {
                    if packet.format != backend.config.format()? {
                        return Err(Error::Malformed);
                    }
                    // Stop a blocked pipe rather than buffering stale audio.
                    tokio::time::timeout(
                        Duration::from_millis(500),
                        write(&mut send, packet.encode().map_err(|_| Error::Malformed)?),
                    )
                    .await
                    .map_err(|_| Error::Timeout)??;
                    last_write = std::time::Instant::now();
                } else {
                    if kind == AudioKind::System
                        && last_write.elapsed() >= Duration::from_millis(500)
                    {
                        tokio::time::timeout(Duration::from_millis(500), write(&mut send, vec![3]))
                            .await
                            .map_err(|_| Error::Timeout)??;
                        last_write = std::time::Instant::now();
                    }
                    tokio::select! {_=backend.source.notified()=>{},_=tick.tick()=>{}}
                }
            }
        };
        tokio::select! {
            result = sending => result.inspect_err(|_| ending.why(HostedEnd::ViewerClosed)),
            _ = read(&mut recv) => {
                ending.why(HostedEnd::ViewerClosed);
                Ok(())
            }
        }
    };
    if *parent.borrow() || *stop.borrow() {
        return Err(Error::NoSession);
    }
    let result = tokio::select! {
        biased;
        _ = parent.changed() => {
            ending.why(HostedEnd::ConnectionEnded);
            Err(Error::NoSession)
        }
        _ = stop.changed() => {
            ending.why(HostedEnd::service_closed(&shared, id));
            Err(Error::NoSession)
        }
        result = run => result,
    };
    // The service closed under the stream for a reason other than its
    // viewer's close: the viewer hears it was withdrawn.
    if *stop.borrow() && HostedEnd::service_closed(&shared, id) == HostedEnd::ServiceClosed {
        crate::ignored!(
            crate::trace::AUDIO,
            "the peer may be gone before it hears why",
            tokio::time::timeout(Duration::from_millis(500), write(&mut send, vec![4])).await
        );
    }
    ending.close(result.as_ref().err().map(Error::name));
    crate::ignored!(
        crate::trace::AUDIO,
        "a stream that cannot finish is closed with its link",
        send.finish()
    );
    result
}
