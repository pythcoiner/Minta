use std::sync::mpsc;

#[allow(async_fn_in_trait)]
pub trait ServiceFn<Message> {
    fn new(
        sender: async_channel::Sender<Message>,
        receiver: mpsc::Receiver<Message>,
        loopback: mpsc::Sender<Message>,
    ) -> Self;
    async fn run(&mut self);
}

#[macro_export]
macro_rules! listener {
    ($Listener:ident, $Message_type:ty, $OutputMessage:ty, $Message:ident) => {
        use async_channel::Receiver;
        use iced::futures::stream::BoxStream;
        use iced::futures::StreamExt;
        use iced_runtime::core::Hasher;
        use iced_runtime::futures::subscription::{EventStream, Recipe};
        use std::hash::Hash;

        pub struct $Listener {
            pub receiver: Receiver<$Message_type>,
        }

        impl Recipe for $Listener {
            type Output = $OutputMessage;
            fn hash(&self, state: &mut Hasher) {
                std::any::TypeId::of::<Self>().hash(state);
            }

            fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<'static, Self::Output> {
                self.receiver.map($Message).boxed()
            }
        }
    };
}
