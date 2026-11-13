# DissCord

A small Discord-style chat system written in Rust: an auth service, chat
servers, and a desktop GUI client (egui/eframe).

## Layout

```
common/   shared wire protocol (auth and chat messages, newline-delimited JSON)
auth/     auth service: owns all accounts and login sessions (SQLite + Argon2)
server/   chat server: channels, messages, presence (SQLite)
client/   desktop GUI client (eframe / egui)
```

One account works on every chat server. The chat servers do not store
accounts. They ask the auth service who a session token belongs to.

## Running

Start three things, each in its own terminal.

1. The auth service. Usage is `auth [ADDR] [DB_PATH]`; the defaults are
   `127.0.0.1:7900` and `auth.db`:

   ```sh
   cargo run -p auth
   ```

2. One or more chat servers. Usage is `server [ADDR] [DB_PATH] [AUTH_ADDR]`;
   the defaults are `127.0.0.1:7878`, `disscord.db` and `127.0.0.1:7900`:

   ```sh
   cargo run -p server -- 127.0.0.1:7878 disscord.db 127.0.0.1:7900
   cargo run -p server -- 127.0.0.1:7879 second.db 127.0.0.1:7900   # optional second server
   ```

3. The GUI client:

   ```sh
   cargo run -p client
   ```

The client finds the auth service at `127.0.0.1:7900`. To use another one, set
`DISSCORD_AUTH=host:port` before starting the client.

Messages, channels and servers' data are stored in each chat server's SQLite
database, so they survive restarts. Accounts are in the auth database.

## Getting started

1. **Log in or register** with your email and password. Registering also asks
   for a display name. Passwords must be at least 8 characters. This is the
   only login; you don't enter a server here.
2. **Add a server**: click **+** in the server rail, give the server a name and
   its address (blank means `127.0.0.1:7878`). Your account joins it straight
   away. The server list is saved between runs.
3. Click a server's icon in the rail to switch to it. Your account works on
   every server you add.

Your login lasts until you log out, so after a restart you log in again.

## Using the client

The layout follows Discord: a server rail on the far left, the channel
sidebar with your user bar at the bottom, the channel header on top, and the
online member list on the right.

- **Channels**: click one in the sidebar to switch. A white dot means that
  channel has new messages you haven't looked at yet.
- **Create channel**: type a name in the sidebar box and press Enter. Channels
  are created on demand.
- **Messages**: type in the bottom bar and press Enter. Consecutive messages
  from the same person are grouped under one header, and date dividers appear
  between days. Hover a message to see its exact time.
- **Members**: the right panel lists everyone currently online.
- **Menus**:
  - Click the server's name at the top of the channel sidebar, or right-click
    its icon in the rail: Create channel, Copy server address, Hide/Show member
    list, Log out. The rail menu also has Remove server.
  - Right-click a channel: Mark as read (when it has unread messages) and Copy
    channel name.
  - Right-click a user's name in a message or the member list: Mention (adds
    `@name` to the message box) and Copy name.
  - Click **More** in the bottom-left user bar: Copy my name and Log out.
- **Log out**: ends your session on the auth service and returns to the login
  screen.
- Channels `#general` and `#random` exist at startup. Channel names are
  lowercased and limited to letters, digits, `-` and `_`.

## Protocol

### Auth service

Send one JSON line per request to the auth service. It replies with one line.

```json
{"type":"register","name":"alice","email":"alice@example.com","password":"correct-horse"}
{"type":"login","email":"alice@example.com","password":"correct-horse"}
{"type":"verify","token":"..."}
{"type":"logout","token":"..."}
```

Replies are `session` (with `token` and `name`), `verified`, `logged_out`, or
`error`.

### Chat servers

Each line is one JSON object with a `type` field. The first message on a
connection must be `connect`, carrying a session token from the auth service:

```json
{"type":"connect","token":"..."}
{"type":"join","channel":"general"}
{"type":"say","text":"hello"}
```

The server answers with `welcome`, or with `error` and closes the connection.
After `welcome` it streams `message` and `system` events for the channel the
client is in, plus `joined` (with up to 200 messages of history).

## Limitations

- Accounts have no email verification and no password reset.
- Passwords are hashed with Argon2, but emails are stored in plain text in the
  auth database, so anyone who can read that file can see them.
- The auth service must be running for anyone to log in or for chat servers to
  accept connections.
- A session token is kept only in the client's memory, so you log in again
  after restarting the client.
- Only one connection per account on each chat server at a time.
- Plain TCP, no TLS.
- Databases from earlier versions keep their old `accounts` and `users`
  tables, which are no longer used. Accounts registered on the old chat
  servers must register again with the auth service.
- The GUI has no message editing, reactions, or direct messages yet.
