use super::*;
use super::events::handle_winit_event;

impl WaylandPlatform {

    pub(super) fn accept_clients(&mut self) -> PlatformResult<()> {
        if let Some(stream) = self.listener.accept().map_err(err)? {
            let client = self.display.handle().insert_client(stream, std::sync::Arc::new(ClientState::default())).map_err(err)?;
            self.clients.push(client);
        }
        Ok(())
    }

    pub(super) fn pump_winit(&mut self) -> PlatformResult<bool> {
        let mut closed = false;
        let state = &mut self.state;
        let output = &self.output;
        let pump = self.winit_events.dispatch_new_events(|event| {
            handle_winit_event(state, output, event, &mut closed);
        });
        if matches!(pump, PumpStatus::Exit(_)) {
            closed = true;
        }
        Ok(closed)
    }
}
