use super::*;

impl<E: EnginePorts> PlatformHost<E> {
    #[must_use]
    pub fn system_snapshot(&self) -> SystemSnapshot {
        self.system.snapshot().clone()
    }

    pub fn set_system_action_handler(&mut self, handler: SystemActionHandler) {
        self.system.set_action_handler(handler);
    }

    /// Dispatches a media key to the default output sink through the system action path.
    /// `cap` is the live volume cap from the shared "raise maximum volume" setting.
    pub fn media_audio_key(
        &mut self,
        key: crate::system::MediaAudioKey,
        cap: u8,
    ) -> FlameResult<()> {
        self.system.media_audio_key(key, cap)
    }

    /// Dispatches a brightness key to the panel backlight through the system action path.
    pub fn media_brightness_key(
        &mut self,
        key: crate::system::MediaBrightnessKey,
    ) -> FlameResult<()> {
        self.system.media_brightness_key(key)
    }

    pub fn system_action(
        &mut self,
        action: SystemAction,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.system.perform_action(action, expected_revision)
    }

    #[must_use]
    pub fn system(&self) -> &SystemService {
        &self.system
    }

    pub fn system_mut(&mut self) -> &mut SystemService {
        &mut self.system
    }
}
