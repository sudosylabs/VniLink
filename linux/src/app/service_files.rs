//! One native Files window's remote backend and transfer queue.
use std::sync::{Arc, Mutex};
use vnilink::{Core, Error, JobSnapshot, RemoteFilesService, ServiceKind, SessionPhase};

pub(crate) struct ServiceFiles {
    core: Arc<Core>,
    peer: String,
    state: Mutex<(bool, Option<Arc<RemoteFilesService>>)>,
    opening: Mutex<()>,
}
impl ServiceFiles {
    pub(crate) fn new(core: Arc<Core>, peer: String) -> Self {
        Self {
            core,
            peer,
            state: Mutex::new((false, None)),
            opening: Mutex::new(()),
        }
    }
    pub(crate) fn file_roots(&self) -> Result<Vec<(u64, std::path::PathBuf)>, Error> {
        self.core.file_roots()
    }
    pub(crate) fn list_dir(
        &self,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<vnilink::DirPage, Error> {
        self.core.list_dir(root, path, offset, limit)
    }
    pub(crate) fn create_dir(&self, root: u64, path: &str, name: &str) -> Result<(), Error> {
        self.core.create_dir(root, path, name)
    }
    pub(crate) fn rename_entry(
        &self,
        root: u64,
        path: &str,
        name: &str,
        token: Option<vnilink::EntryToken>,
        new_name: &str,
    ) -> Result<(), Error> {
        self.core.rename_entry(root, path, name, token, new_name)
    }
    pub(crate) fn owner_stamp(&self) -> Option<(u64, u64)> {
        let state = self.state.lock().unwrap();
        if state.0 {
            return None;
        }
        state
            .1
            .as_ref()
            .map(|files| (files.info().id, files.info().generation))
    }
    pub(crate) fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        if let Some(files) = &state.1 {
            files.close();
        }
    }
    fn current(&self) -> Result<Arc<RemoteFilesService>, Error> {
        let state = self.state.lock().unwrap();
        if state.0 {
            return Err(Error::NoSession);
        }
        state.1.clone().ok_or(Error::NoSession)
    }
    fn files(&self, id: u64, generation: u64) -> Result<Arc<RemoteFilesService>, Error> {
        let _opening = self.opening.lock().unwrap();
        let existing = {
            let state = self.state.lock().unwrap();
            if state.0 {
                return Err(Error::NoSession);
            }
            state.1.clone()
        };
        if let Some(files) = &existing {
            if files.info().connection_id == id
                && files.info().generation == generation
                && !files.is_cancelled()
            {
                return Ok(files.clone());
            }
        }
        let session = self.core.controller_session(id)?;
        if session.generation != generation
            || session.phase != SessionPhase::Active
            || session.peer_id != self.peer
        {
            return Err(Error::StaleSession);
        }
        if let Some(files) = existing {
            // Reconnect may have advanced the stored record. Close its current
            // epoch before admitting this window's replacement backend.
            if let Ok(info) = self.core.controller_service(files.info().id) {
                self.core
                    .close_controller_service(info.id, info.generation)?;
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let info = loop {
            match self
                .core
                .attach_controller_service(id, generation, ServiceKind::Files)
            {
                Ok(info) => break info,
                Err(Error::Busy) if std::time::Instant::now() < deadline => {
                    if self.state.lock().unwrap().0 {
                        return Err(Error::NoSession);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        };
        let files = match self.core.open_files_service(info.id, info.generation) {
            Ok(files) => Arc::new(files),
            Err(error) => {
                let _ = self.core.close_controller_service(info.id, info.generation);
                return Err(error);
            }
        };
        let mut state = self.state.lock().unwrap();
        if state.0 {
            files.close();
            return Err(Error::NoSession);
        }
        state.1 = Some(files.clone());
        Ok(files)
    }
    pub(crate) fn fs_list(
        &self,
        id: u64,
        generation: u64,
        root: u64,
        path: &str,
        offset: u64,
        limit: u16,
    ) -> Result<vnilink::DirPage, Error> {
        self.files(id, generation)?.list(root, path, offset, limit)
    }
    pub(crate) fn fs_mkdir(
        &self,
        id: u64,
        generation: u64,
        root: u64,
        path: &str,
        name: &str,
    ) -> Result<(), Error> {
        self.files(id, generation)?.mkdir(root, path, name)
    }
    pub(crate) fn fs_rename(
        &self,
        id: u64,
        generation: u64,
        root: u64,
        path: &str,
        name: &str,
        token: Option<vnilink::EntryToken>,
        new_name: &str,
    ) -> Result<(), Error> {
        self.files(id, generation)?
            .rename(root, path, name, token, new_name)
    }
    pub(crate) fn upload(
        &self,
        id: u64,
        generation: u64,
        src: u64,
        path: &str,
        dst: u64,
        destination: &str,
    ) -> Result<u64, Error> {
        let files = self.files(id, generation)?;
        let info = files.info();
        self.core
            .files_service_upload(info.id, info.generation, src, path, dst, destination)
    }
    pub(crate) fn download(
        &self,
        id: u64,
        generation: u64,
        src: u64,
        path: &str,
        dst: u64,
        destination: &str,
    ) -> Result<u64, Error> {
        let files = self.files(id, generation)?;
        let info = files.info();
        self.core
            .files_service_download(info.id, info.generation, src, path, dst, destination)
    }
    pub(crate) fn job_list(&self) -> Result<Vec<JobSnapshot>, Error> {
        let state = self.state.lock().unwrap();
        if state.0 {
            return Err(Error::NoSession);
        }
        let files = state.1.clone();
        drop(state);
        match files {
            Some(files) => self
                .core
                .files_service_jobs(files.info().id, files.info().generation),
            None => Ok(Vec::new()),
        }
    }
    pub(crate) fn job_poll(&self, job: u64) -> Result<JobSnapshot, Error> {
        let files = self.current()?;
        let info = files.info();
        self.core
            .files_service_job_poll(info.id, info.generation, job)
    }
    pub(crate) fn job_cancel(&self, job: u64) -> Result<JobSnapshot, Error> {
        let files = self.current()?;
        let info = files.info();
        self.core
            .files_service_job_cancel(info.id, info.generation, job)
    }
    pub(crate) fn job_resume(&self, job: u64) -> Result<JobSnapshot, Error> {
        let files = self.current()?;
        let info = files.info();
        self.core
            .files_service_job_resume(info.id, info.generation, job)
    }
    pub(crate) fn job_resolve(
        &self,
        job: u64,
        choice: vnilink::ConflictChoice,
        remaining: bool,
    ) -> Result<JobSnapshot, Error> {
        let files = self.current()?;
        let info = files.info();
        self.core
            .files_service_job_resolve(info.id, info.generation, job, choice, remaining)
    }
    pub(crate) fn job_forget(&self, job: u64) -> Result<(), Error> {
        let files = self.current()?;
        let info = files.info();
        self.core
            .files_service_job_forget(info.id, info.generation, job)
    }
}
