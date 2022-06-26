use std::collections::HashMap;
use std::sync::Arc;

use futures::future::{AbortHandle, Abortable};
use futures_util::{stream::SplitStream, StreamExt};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use shape_evolution::random_shape::RandomCircle;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Duration;
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::ws::{Message, WebSocket};
use warp::{http::Response, Filter};

mod res;
use res::{InboundWsEvent, OutboundWsEvent};

mod handlers;

/// Public facing info about a player - we aren't worried about
/// other players knowing this data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerInfo {
    name: String,
    public_id: String,
    is_host: bool,
    has_answer: bool,
}
#[derive(Debug, Serialize)]
pub struct Player {
    private_id: String,
    info: PlayerInfo,
    #[serde(skip_serializing)]
    sender: mpsc::UnboundedSender<Message>,
}
impl Player {
    pub fn new(name: String, is_host: bool, sender: mpsc::UnboundedSender<Message>) -> Self {
        let public_id = uuid::Uuid::new_v4().to_string();
        let private_id = uuid::Uuid::new_v4().to_string();
        let info = PlayerInfo {
            name,
            public_id,
            is_host,
            has_answer: false,
        };
        Player {
            private_id,
            info,
            sender,
        }
    }
}

type Players = Vec<Player>;

pub struct Room {
    id: String,
    players: Players,
    circles: Vec<RandomCircle>,

    image_dimensions: Option<(u32, u32)>,
    answer: Option<String>,
    image_data: Option<Vec<u8>>,

    cleanup_abort_handle: Option<AbortHandle>,
}
impl Room {
    pub fn new(id: String) -> Self {
        Room {
            id,
            players: Players::default(),
            circles: Vec::new(),

            image_dimensions: None,
            answer: None,
            image_data: None,

            cleanup_abort_handle: None,
        }
    }

    pub fn wait_then_cleanup(&mut self, rooms: Rooms, duration: Duration) {
        println!("Deleting room {} in {:?}", &self.id, &duration);
        // Start an abortable cleanup task for this room.
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.cleanup_abort_handle = Some(abort_handle);

        let room_id = self.id.clone();
        let future = Abortable::new(
            async move {
                tokio::time::sleep(duration).await;
                delete_room(&rooms, &room_id).await;
            },
            abort_registration,
        );
        tokio::task::spawn(future);
    }

    pub fn cancel_cleanup(&mut self) {
        if let Some(abort_handle) = &self.cleanup_abort_handle {
            println!("Cancelling cleanup for room {}", &self.id);
            abort_handle.abort();
            self.cleanup_abort_handle = None;
        }
    }

    pub fn insert_player(&mut self, player: Player) {
        self.players.push(player);
    }

    pub fn get_player(&self, private_id: &str) -> Option<&Player> {
        self.players.iter().find(|p| p.private_id == private_id)
    }

    pub fn get_player_mut(&mut self, private_id: &str) -> Option<&mut Player> {
        self.players.iter_mut().find(|p| p.private_id == private_id)
    }
}

type Rooms = Arc<RwLock<HashMap<String, Arc<RwLock<Room>>>>>;

fn room_filter(
    rooms: Rooms,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let filter = warp::any().map(move || rooms.clone());
    let cors = warp::cors().allow_any_origin();

    // GET /room -> creates a room and returns a path for joining it
    warp::path("room")
        .and(filter)
        .and_then(|rooms| async move { new_room(rooms).await })
        .with(cors)
}

fn join_filter(
    rooms: Rooms,
) -> impl Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    let filter = warp::any().map(move || rooms.clone());

    // GET /join -> websocket upgrade
    warp::path!("join" / String)
        // The `ws()` filter will prepare Websocket handshake...
        .and(warp::ws())
        .and(filter)
        .map(|room_id: String, ws: warp::ws::Ws, rooms| {
            // This will call our function if the handshake succeeds.
            ws.on_upgrade(move |socket| player_connected(socket, rooms, room_id))
        })
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let rooms = Rooms::default();

    let room = room_filter(rooms.clone());
    let join = join_filter(rooms.clone());

    let routes = room.or(join);

    warp::serve(routes).run(([0, 0, 0, 0], 3001)).await;
}

async fn new_room(rooms: Rooms) -> Result<Response<String>, warp::Rejection> {
    // Generate a 7 character alphanumeric id for the room
    let room_id: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();

    let mut new_room = Room::new(room_id.clone());

    new_room.wait_then_cleanup(rooms.clone(), Duration::from_secs(5 * 60));

    rooms
        .write()
        .await
        .insert(room_id.clone(), Arc::new(RwLock::new(new_room)));
    eprintln!("New room: {}", &room_id);

    Ok(Response::builder()
        .body(format!("/join/{}/", &room_id))
        .unwrap())
}

// TODO: test this function
async fn wait_for_name(client_ws_rcv: &mut SplitStream<WebSocket>) -> Option<String> {
    let result = client_ws_rcv.next().await?;
    let msg = result.ok()?;
    let text = msg.to_str().ok()?;

    match serde_json::from_str::<InboundWsEvent>(text) {
        Ok(event) => {
            if let InboundWsEvent::PlayerName(name) = event {
                Some(name.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn make_hint(answer: &str) -> String {
    answer
        .chars()
        .map(|c| if c != ' ' { '_' } else { c })
        .collect()
}

async fn player_connected(ws: WebSocket, rooms: Rooms, room_id: String) {
    // First check that the provided room id matches an existing room.
    let room = {
        if let Some(room) = rooms.read().await.get(&room_id) {
            room.clone()
        } else {
            // The room doesn't exist, notify the browser somehow
            todo!();
        }
    };

    // Split the socket into a sender and receiver of messages.
    let (client_ws_sender, mut client_ws_rcv) = ws.split();
    let (client_sender, client_rcv) = mpsc::unbounded_channel();
    let client_rcv = UnboundedReceiverStream::new(client_rcv);

    tokio::task::spawn(client_rcv.map(Ok).forward(client_ws_sender));

    // Wait for player to send a name before continuing with connection
    let name = match wait_for_name(&mut client_ws_rcv).await {
        Some(name) => name,
        None => {
            return;
        }
    };

    {
        let room = room.read().await;
        // Send room's current image dimensions (if they exist) to the new player
        // TODO: consider moving this
        if let (Some(dimensions), Some(answer)) = (room.image_dimensions, room.answer.clone()) {
            let answer_hint = make_hint(&answer);
            client_sender
                .send(Message::text(
                    serde_json::to_string(&OutboundWsEvent::NewImage {
                        dimensions,
                        answer_hint: &answer_hint,
                    })
                    .unwrap(),
                ))
                .expect("Error sending new-image");
        }
    }

    // TODO: this should be done in the same lock as when the player is added to the room.
    let is_host = {
        let players = &room.read().await.players;
        players.is_empty()
    };

    let player = Player::new(name, is_host, client_sender);

    connect_player(player, rooms, room, client_ws_rcv).await;
}

async fn connect_player(
    player: Player,
    rooms: Rooms,
    room: Arc<RwLock<Room>>,
    mut client_ws_rcv: SplitStream<WebSocket>,
) {
    let private_id = player.private_id.clone();
    {
        let mut room = room.write().await;
        // Cancel the room's cleanup process
        room.cancel_cleanup();

        // Add the player to the room
        room.players.push(player);

        let player = room
            .players
            .last()
            .expect("There should always be a player after the push");

        println!("Player {} joined room {}", player.private_id, &room.id);

        // Tell the player what their id is
        send_private_info(player);
        broadcast_player_list(&room);
        send_current_circles(player, &room);
        broadcast_server_message(&format!("{} joined", player.info.name), &room);
    }

    // Every time the host sends a message, broadcast it to
    // all other users
    while let Some(result) = client_ws_rcv.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("websocket error(uid={}): {}", private_id, e);
                break;
            }
        };
        handle_user_message(&private_id, msg, room.clone()).await;
    }

    // user_ws_rx stream will keep processing as long as the user stays
    // connected. Once they disconnect, then...
    handlers::handle_user_disconnected(&private_id, rooms, room).await;
}

fn handle_round_over(room: &mut Room) {
    // Set everyone's has_answer to false
    room.players
        .iter_mut()
        .for_each(|mut p| p.info.has_answer = false);

    // Update the host
    let host_index = room
        .players
        .iter()
        .position(|p| p.info.is_host)
        .expect("no host in room");

    room.players[host_index].info.is_host = false;

    let host_index = (host_index + 1) % room.players.len();
    room.players[host_index].info.is_host = true;

    // Broadcast player info and source image when we're done
    broadcast_source_image(room);
    broadcast_player_list(room);
    if let Some(answer) = &room.answer {
        broadcast_server_message(&format!(r#"The answer was "{}""#, answer), &room);
    } else {
        eprintln!("Error: room has no answer");
    }
}

fn handle_chat_message(
    message: &str,
    private_id: &str,
    room: &mut Room,
) -> Result<(), &'static str> {
    if message.is_empty() {
        return Err("Empty message");
    }

    if room.answer == Some(message.to_lowercase()) {
        let mut player = room.get_player_mut(private_id).ok_or("Player not found")?;
        // Player guessed correctly
        player.info.has_answer = true;

        send_player_correct_answer(player, message);
        broadcast_server_message(&format!("{} got it right", player.info.name), &room);
        broadcast_player_list(&room);

        // Check if all non-host players have finished
        let round_over = room
            .players
            .iter()
            .filter(|p| !p.info.is_host)
            .all(|p| p.info.has_answer);
        if round_over {
            handle_round_over(room);
        }
    } else {
        let player = room.get_player(private_id).ok_or("Player not found")?;
        let event = OutboundWsEvent::ChatMessage {
            name: &player.info.name,
            text: message,
        };
        broadcast_ws_event(event, room);
    }

    Ok(())
}

async fn handle_user_message(private_id: &str, msg: Message, room: Arc<RwLock<Room>>) {
    // Skip any non-Text messages
    if let Ok(msg) = msg.to_str() {
        handlers::handle_text_message(private_id, msg, room).await;
    } else {
        let msg = msg.into_bytes();
        handlers::handle_binary_message(private_id, msg, room).await;
    };
}

async fn delete_room(rooms: &Rooms, room_id: &str) {
    {
        let mut rooms = rooms.write().await;
        rooms.remove(room_id);
    }
    eprintln!("Deleted room {}", &room_id);
}

// Send a WsEvent to a single player
fn send_ws_event(event: OutboundWsEvent, player: &Player) {
    let message = serde_json::to_string(&event).expect("failed to serialize event");
    player.sender.send(Message::text(&message));
}

fn send_current_circles(player: &Player, room: &Room) {
    // No need to send an empty sequence
    if room.circles.is_empty() {
        return;
    }

    let event = OutboundWsEvent::CircleSequence(room.circles.clone());
    send_ws_event(event, player);
}

fn send_private_info(player: &Player) {
    let event = OutboundWsEvent::PrivateInfo(player);
    send_ws_event(event, player);
}

fn send_player_correct_answer(player: &Player, answer: &str) {
    let event = OutboundWsEvent::Answer(answer);
    send_ws_event(event, player);
}

// Broadcast a WsEvent to every player in a room
fn broadcast_ws_event(event: OutboundWsEvent, room: &Room) {
    let message = serde_json::to_string(&event).unwrap();
    for p in room.players.iter() {
        if let Err(_disconnected) = p.sender.send(Message::text(&message)) {
            // The tx is disconnected, our `user_disconnected` code
            // should be happening in another task, nothing more to
            // do here.
        }
    }
}

fn broadcast_player_list(room: &Room) {
    let player_list = OutboundWsEvent::PlayerList(room.players.iter().map(|p| &p.info).collect());

    broadcast_ws_event(player_list, room);
}

fn broadcast_server_message(message: &str, room: &Room) {
    let server_message = OutboundWsEvent::ServerMessage(message);
    broadcast_ws_event(server_message, room);
}

fn broadcast_source_image(room: &Room) {
    eprintln!("sending binary");
    if let Some(image_data) = &room.image_data {
        eprintln!("data length is {}", image_data.len());
        for p in room.players.iter() {
            let res = p.sender.send(Message::binary(image_data.clone()));
            eprintln!("{:?}", res);
        }
    }
}


#[cfg(test)]
mod tests {
    use crate::{join_filter, new_room, room_filter, OutboundWsEvent, Rooms};
    use image::Rgba;
    use shape_evolution::random_shape::RandomCircle;
    use warp::Filter;

    #[tokio::test]
    async fn test_new_room() {
        let rooms = Rooms::default();

        let room = room_filter(rooms.clone());

        assert_eq!(rooms.read().await.len(), 0);

        let res = warp::test::request().path("/room").reply(&room).await;

        assert_eq!(res.status(), 200);
        assert_eq!(rooms.read().await.len(), 1);
    }

    #[ignore]
    #[tokio::test]
    async fn test_room_cleanup() {
        let rooms = Rooms::default();

        let room = room_filter(rooms.clone());

        assert_eq!(rooms.read().await.len(), 0);

        let res = warp::test::request().path("/room").reply(&room).await;

        assert_eq!(rooms.read().await.len(), 1);

        tokio::time::sleep(tokio::time::Duration::from_millis(5100)).await;

        assert_eq!(rooms.read().await.len(), 0);
    }

    #[tokio::test]
    async fn test_player_join() {
        let rooms = Rooms::default();

        let room = room_filter(rooms.clone());
        let join = join_filter(rooms.clone());

        assert_eq!(rooms.read().await.len(), 0);

        let res = warp::test::request().path("/room").reply(&room).await;
        let join_path = std::str::from_utf8(res.body()).expect("body was not utf8");

        let mut ws = warp::test::ws()
            .path(join_path)
            .handshake(join)
            .await
            .expect("handshake");

        ws.send_text("{\"PlayerName\": \"Alice\"}").await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        {
            let rooms = rooms.read().await;
            assert_eq!(rooms.len(), 1);
            let room = rooms.values().next().unwrap();
            let room = room.read().await;
            assert_eq!(room.players.len(), 1);
            let player = &room.players[0];
            assert_eq!(player.info.name, "Alice");
        }
    }

    #[tokio::test]
    async fn test_circles_clear() {
        let rooms = Rooms::default();

        let room = room_filter(rooms.clone());
        let join = join_filter(rooms.clone());

        assert_eq!(rooms.read().await.len(), 0);

        let res = warp::test::request().path("/room").reply(&room).await;
        let join_path = std::str::from_utf8(res.body()).expect("body was not utf8");

        let mut ws = warp::test::ws()
            .path(join_path)
            .handshake(join)
            .await
            .expect("handshake");

        ws.send_text("{\"PlayerName\": \"Alice\"}").await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        ws.send_text("{\"NewImage\": {\"dimensions\": [100, 200], \"answer\": \"foo\"}}")
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let circle = RandomCircle {
            imgx: 100,
            imgy: 200,
            center: (50, 50),
            radius: 20,
            color: Rgba([255, 0, 0, 255]),
        };
        ws.send_text(serde_json::to_string(&OutboundWsEvent::Circle(circle)).unwrap())
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        {
            let rooms = rooms.read().await;
            let room = rooms.values().next().unwrap();
            let room = room.read().await;
            assert_eq!(room.circles.len(), 1);
        }

        ws.send_text("{\"NewImage\": {\"dimensions\": [100, 200], \"answer\": \"foo\"}}")
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        {
            let rooms = rooms.read().await;
            let room = rooms.values().next().unwrap();
            let room = room.read().await;
            assert_eq!(room.circles.len(), 0);
        }
    }
}
