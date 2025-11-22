use hanzo_messages::hanzo_message::hanzo_message::{MessageBody, MessageData, HanzoMessage};

#[allow(dead_code)]
pub fn print_content_time_messages(messages: Vec<HanzoMessage>) {
    for message in &messages {
        match &message.body {
            MessageBody::Unencrypted(body) => match &body.message_data {
                MessageData::Unencrypted(data) => {
                    println!(
                        "Content: {}, Time: {}",
                        data.message_raw_content, message.external_metadata.scheduled_time
                    );
                }
                _ => println!("Message data is encrypted"),
            },
            _ => println!("Message body is encrypted"),
        }
    }
}
