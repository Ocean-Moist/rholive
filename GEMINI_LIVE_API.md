Live API

Preview: The Live API is in preview.
The Live API enables low-latency bidirectional voice and video interactions with Gemini, letting you talk to Gemini live while also streaming video input or sharing your screen. Using the Live API, you can provide end users with the experience of natural, human-like voice conversations.

You can try the Live API in Google AI Studio. To use the Live API in Google AI Studio, select Stream.

How the Live API works
Streaming
The Live API uses a streaming model over a WebSocket connection. When you interact with the API, a persistent connection is created. Your input (audio, video, or text) is streamed continuously to the model, and the model's response (text or audio) is streamed back in real-time over the same connection.

This bidirectional streaming ensures low latency and supports features such as voice activity detection, tool usage, and speech generation.

Live API Overview

For more information about the underlying WebSockets API, see the WebSockets API reference.

Warning: It is unsafe to insert your API key into client-side JavaScript or TypeScript code. Use server-side deployments for accessing the Live API in production.
Output generation
The Live API processes multimodal input (text, audio, video) to generate text or audio in real-time. It comes with a built-in mechanism to generate audio and depending on the model version you use, it uses one of the two audio generation methods:

Half cascade: The model receives native audio input and uses a specialized model cascade of distinct models to process the input and to generate audio output.
Native: Gemini 2.5 introduces native audio generation, which directly generates audio output, providing a more natural sounding audio, more expressive voices, more awareness of additional context, e.g., tone, and more proactive responses.
Building with Live API
Before you begin building with the Live API, choose the audio generation approach that best fits your needs.

Establishing a connection
The following example shows how to create a connection with an API key:

Python
JavaScript

import asyncio
from google import genai

client = genai.Client(api_key="GEMINI_API_KEY")

model = "gemini-2.0-flash-live-001"
config = {"response_modalities": ["TEXT"]}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
print("Session started")

if __name__ == "__main__":
asyncio.run(main())
Note: You can only set one modality in the response_modalities field. This means that you can configure the model to respond with either text or audio, but not both in the same session.
Sending and receiving text
Here's how you can send and receive text:

Python
JavaScript

import asyncio
from google import genai

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["TEXT"]}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
message = "Hello, how are you?"
await session.send_client_content(
turns={"role": "user", "parts": [{"text": message}]}, turn_complete=True
)

        async for response in session.receive():
            if response.text is not None:
                print(response.text, end="")

if __name__ == "__main__":
asyncio.run(main())
Sending and receiving audio
You can send audio by converting it to 16-bit PCM, 16kHz, mono format. This example reads a WAV file and sends it in the correct format:

Python
JavaScript

# Test file: https://storage.googleapis.com/generativeai-downloads/data/16000.wav
# Install helpers for converting files: pip install librosa soundfile
import asyncio
import io
from pathlib import Path
from google import genai
from google.genai import types
import soundfile as sf
import librosa

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["TEXT"]}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:

        buffer = io.BytesIO()
        y, sr = librosa.load("sample.wav", sr=16000)
        sf.write(buffer, y, sr, format='RAW', subtype='PCM_16')
        buffer.seek(0)
        audio_bytes = buffer.read()

        # If already in correct format, you can use this:
        # audio_bytes = Path("sample.pcm").read_bytes()

        await session.send_realtime_input(
            audio=types.Blob(data=audio_bytes, mime_type="audio/pcm;rate=16000")
        )

        async for response in session.receive():
            if response.text is not None:
                print(response.text)

if __name__ == "__main__":
asyncio.run(main())
You can receive audio by setting AUDIO as response modality. This example saves the received data as WAV file:

Python
JavaScript

import asyncio
import wave
from google import genai

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["AUDIO"]}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
wf = wave.open("audio.wav", "wb")
wf.setnchannels(1)
wf.setsampwidth(2)
wf.setframerate(24000)

        message = "Hello how are you?"
        await session.send_client_content(
            turns={"role": "user", "parts": [{"text": message}]}, turn_complete=True
        )

        async for response in session.receive():
            if response.data is not None:
                wf.writeframes(response.data)

            # Un-comment this code to print audio data info
            # if response.server_content.model_turn is not None:
            #      print(response.server_content.model_turn.parts[0].inline_data.mime_type)

        wf.close()

if __name__ == "__main__":
asyncio.run(main())
Audio formats
Audio data in the Live API is always raw, little-endian, 16-bit PCM. Audio output always uses a sample rate of 24kHz. Input audio is natively 16kHz, but the Live API will resample if needed so any sample rate can be sent. To convey the sample rate of input audio, set the MIME type of each audio-containing Blob to a value like audio/pcm;rate=16000.

Receiving audio transcriptions
You can enable transcription of the model's audio output by sending output_audio_transcription in the setup config. The transcription language is inferred from the model's response.


import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["AUDIO"],
"output_audio_transcription": {}
}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
message = "Hello? Gemini are you there?"

        await session.send_client_content(
            turns={"role": "user", "parts": [{"text": message}]}, turn_complete=True
        )

        async for response in session.receive():
            if response.server_content.model_turn:
                print("Model turn:", response.server_content.model_turn)
            if response.server_content.output_transcription:
                print("Transcript:", response.server_content.output_transcription.text)


if __name__ == "__main__":
asyncio.run(main())
You can enable transcription of the audio input by sending input_audio_transcription in setup config.


import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["TEXT"],
"realtime_input_config": {
"automatic_activity_detection": {"disabled": True},
"activity_handling": "NO_INTERRUPTION",
},
"input_audio_transcription": {},
}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
audio_data = Path("sample.pcm").read_bytes()

        await session.send_realtime_input(activity_start=types.ActivityStart())
        await session.send_realtime_input(
            audio=types.Blob(data=audio_data, mime_type='audio/pcm;rate=16000')
        )
        await session.send_realtime_input(activity_end=types.ActivityEnd())

        async for msg in session.receive():
            if msg.server_content.input_transcription:
                print('Transcript:', msg.server_content.input_transcription.text)

if __name__ == "__main__":
asyncio.run(main())
Streaming audio and video
To see an example of how to use the Live API in a streaming audio and video format, run the "Live API - Get Started" file in the cookbooks repository:

View on GitHub

System instructions
System instructions let you steer the behavior of a model based on your specific needs and use cases. System instructions can be set in the setup configuration and will remain in effect for the entire session.


from google.genai import types

config = {
"system_instruction": types.Content(
parts=[
types.Part(
text="You are a helpful assistant and answer in a friendly tone."
)
]
),
"response_modalities": ["TEXT"],
}
Incremental content updates
Use incremental updates to send text input, establish session context, or restore session context. For short contexts you can send turn-by-turn interactions to represent the exact sequence of events:

Python
JSON

turns = [
{"role": "user", "parts": [{"text": "What is the capital of France?"}]},
{"role": "model", "parts": [{"text": "Paris"}]},
]

await session.send_client_content(turns=turns, turn_complete=False)

turns = [{"role": "user", "parts": [{"text": "What is the capital of Germany?"}]}]

await session.send_client_content(turns=turns, turn_complete=True)
For longer contexts it's recommended to provide a single message summary to free up the context window for subsequent interactions.

Changing voice and language
The Live API supports the following voices: Puck, Charon, Kore, Fenrir, Aoede, Leda, Orus, and Zephyr.

To specify a voice, set the voice name within the speechConfig object as part of the session configuration:

Python
JSON

from google.genai import types

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
speech_config=types.SpeechConfig(
voice_config=types.VoiceConfig(
prebuilt_voice_config=types.PrebuiltVoiceConfig(voice_name="Kore")
)
)
)
Note: If you're using the generateContent API, the set of available voices is slightly different. See the audio generation guide for generateContent audio generation voices.
The Live API supports multiple languages.

To change the language, set the language code within the speechConfig object as part of the session configuration:


from google.genai import types

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
speech_config=types.SpeechConfig(
language_code="de-DE",
)
)
Note: Native audio output models automatically choose the appropriate language and don't support explicitly setting the language code.
Native audio output
Through the Live API, you can also access models that allow for native audio output in addition to native audio input. This allows for higher quality audio outputs with better pacing, voice naturalness, verbosity, and mood.

Native audio output is supported by the following native audio models:

gemini-2.5-flash-preview-native-audio-dialog
gemini-2.5-flash-exp-native-audio-thinking-dialog
Note: Native audio models currently have limited tool use support. See Overview of supported tools for details.
How to use native audio output
To use native audio output, configure one of the native audio models and set response_modalities to AUDIO.

See Sending and receiving audio for a full example.

Python
JavaScript

model = "gemini-2.5-flash-preview-native-audio-dialog"
config = types.LiveConnectConfig(response_modalities=["AUDIO"])

async with client.aio.live.connect(model=model, config=config) as session:
# Send audio input and receive audio
Affective dialog
This feature lets Gemini adapt its response style to the input expression and tone.

To use affective dialog, set the api version to v1alpha and set enable_affective_dialog to truein the setup message:

Python
JavaScript

client = genai.Client(api_key="GOOGLE_API_KEY", http_options={"api_version": "v1alpha"})

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
enable_affective_dialog=True
)
Note that affective dialog is currently only supported by the native audio output models.

Proactive audio
When this feature is enabled, Gemini can proactively decide not to respond if the content is not relevant.

To use it, set the api version to v1alpha and configure the proactivity field in the setup message and set proactive_audio to true:

Python
JavaScript

client = genai.Client(api_key="GOOGLE_API_KEY", http_options={"api_version": "v1alpha"})

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
proactivity={'proactive_audio': True}
)
Note that proactive audio is currently only supported by the native audio output models.

Native audio output with thinking
Native audio output supports thinking capabilities, available via a separate model gemini-2.5-flash-exp-native-audio-thinking-dialog.

See Sending and receiving audio for a full example.

Python
JavaScript

model = "gemini-2.5-flash-exp-native-audio-thinking-dialog"
config = types.LiveConnectConfig(response_modalities=["AUDIO"])

async with client.aio.live.connect(model=model, config=config) as session:
# Send audio input and receive audio
Tool use with Live API
You can define tools such as Function calling, Code execution, and Google Search with the Live API.

To see examples of all tools in the Live API, run the "Live API Tools" cookbook:

View on GitHub

Overview of supported tools
Here's a brief overview of the available tools for each model:

Tool	Cascaded models
gemini-2.0-flash-live-001	gemini-2.5-flash-preview-native-audio-dialog	gemini-2.5-flash-exp-native-audio-thinking-dialog
Search	Yes	Yes	Yes
Function calling	Yes	Yes	No
Code execution	Yes	No	No
Url context	Yes	No	No
Function calling
You can define function declarations as part of the session configuration. See the Function calling tutorial to learn more.

After receiving tool calls, the client should respond with a list of FunctionResponse objects using the session.send_tool_response method.

Note: Unlike the generateContent API, the Live API doesn't support automatic tool response handling. You must handle tool responses manually in your client code.

import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

# Simple function definitions
turn_on_the_lights = {"name": "turn_on_the_lights"}
turn_off_the_lights = {"name": "turn_off_the_lights"}

tools = [{"function_declarations": [turn_on_the_lights, turn_off_the_lights]}]
config = {"response_modalities": ["TEXT"], "tools": tools}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
prompt = "Turn on the lights please"
await session.send_client_content(turns={"parts": [{"text": prompt}]})

        async for chunk in session.receive():
            if chunk.server_content:
                if chunk.text is not None:
                    print(chunk.text)
            elif chunk.tool_call:
                function_responses = []
                for fc in tool_call.function_calls:
                    function_response = types.FunctionResponse(
                        id=fc.id,
                        name=fc.name,
                        response={ "result": "ok" } # simple, hard-coded function response
                    )
                    function_responses.append(function_response)

                await session.send_tool_response(function_responses=function_responses)


if __name__ == "__main__":
asyncio.run(main())
From a single prompt, the model can generate multiple function calls and the code necessary to chain their outputs. This code executes in a sandbox environment, generating subsequent BidiGenerateContentToolCall messages.

Asynchronous function calling
By default, the execution pauses until the results of each function call are available, which ensures sequential processing. It means you won't be able to continue interacting with the model while the functions are being run.

If you don't want to block the conversation, you can tell the model to run the functions asynchronously.

To do so, you first need to add a behavior to the function definitions:


# Non-blocking function definitions
turn_on_the_lights = {"name": "turn_on_the_lights", "behavior": "NON_BLOCKING"} # turn_on_the_lights will run asynchronously
turn_off_the_lights = {"name": "turn_off_the_lights"} # turn_off_the_lights will still pause all interactions with the model
NON-BLOCKING will ensure the function will run asynchronously while you can continue interacting with the model.

Then you need to tell the model how to behave when it receives the FunctionResponse using the scheduling parameter. It can either:

Interrupt what it's doing and tell you about the response it got right away (scheduling="INTERRUPT"),
Wait until it's finished with what it's currently doing (scheduling="WHEN_IDLE"),
Or do nothing and use that knowledge later on in the discussion (scheduling="SILENT")

# Non-blocking function definitions
function_response = types.FunctionResponse(
id=fc.id,
name=fc.name,
response={
"result": "ok",
"scheduling": "INTERRUPT" # Can also be WHEN_IDLE or SILENT
}
)
Code execution
You can define code execution as part of the session configuration. See the Code execution tutorial to learn more.


import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

tools = [{'code_execution': {}}]
config = {"response_modalities": ["TEXT"], "tools": tools}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
prompt = "Compute the largest prime palindrome under 100000."
await session.send_client_content(turns={"parts": [{"text": prompt}]})

        async for chunk in session.receive():
            if chunk.server_content:
                if chunk.text is not None:
                    print(chunk.text)
            
                model_turn = chunk.server_content.model_turn
                if model_turn:
                    for part in model_turn.parts:
                      if part.executable_code is not None:
                        print(part.executable_code.code)

                      if part.code_execution_result is not None:
                        print(part.code_execution_result.output)

if __name__ == "__main__":
asyncio.run(main())
Grounding with Google Search
You can enable Grounding with Google Search as part of the session configuration. See the Grounding tutorial to learn more.


import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

tools = [{'google_search': {}}]
config = {"response_modalities": ["TEXT"], "tools": tools}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
prompt = "When did the last Brazil vs. Argentina soccer match happen?"
await session.send_client_content(turns={"parts": [{"text": prompt}]})

        async for chunk in session.receive():
            if chunk.server_content:
                if chunk.text is not None:
                    print(chunk.text)

                # The model might generate and execute Python code to use Search
                model_turn = chunk.server_content.model_turn
                if model_turn:
                    for part in model_turn.parts:
                      if part.executable_code is not None:
                        print(part.executable_code.code)

                      if part.code_execution_result is not None:
                        print(part.code_execution_result.output)

if __name__ == "__main__":
asyncio.run(main())
Combining multiple tools
You can combine multiple tools within the Live API:


prompt = """
Hey, I need you to do three things for me.

1. Compute the largest prime palindrome under 100000.
2. Then use Google Search to look up information about the largest earthquake in California the week of Dec 5 2024?
3. Turn on the lights

Thanks!
"""

tools = [
{"google_search": {}},
{"code_execution": {}},
{"function_declarations": [turn_on_the_lights, turn_off_the_lights]},
]

config = {"response_modalities": ["TEXT"], "tools": tools}
Handling interruptions
Users can interrupt the model's output at any time. When Voice activity detection (VAD) detects an interruption, the ongoing generation is canceled and discarded. Only the information already sent to the client is retained in the session history. The server then sends a BidiGenerateContentServerContent message to report the interruption.

In addition, the Gemini server discards any pending function calls and sends a BidiGenerateContentServerContent message with the IDs of the canceled calls.


async for response in session.receive():
if response.server_content.interrupted is True:
# The generation was interrupted
Voice activity detection (VAD)
You can configure or disable voice activity detection (VAD).

Using automatic VAD
By default, the model automatically performs VAD on a continuous audio input stream. VAD can be configured with the realtimeInputConfig.automaticActivityDetection field of the setup configuration.

When the audio stream is paused for more than a second (for example, because the user switched off the microphone), an audioStreamEnd event should be sent to flush any cached audio. The client can resume sending audio data at any time.


# example audio file to try:
# URL = "https://storage.googleapis.com/generativeai-downloads/data/hello_are_you_there.pcm"
# !wget -q $URL -O sample.pcm
import asyncio
from pathlib import Path
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

config = {"response_modalities": ["TEXT"]}

async def main():
async with client.aio.live.connect(model=model, config=config) as session:
audio_bytes = Path("sample.pcm").read_bytes()

        await session.send_realtime_input(
            audio=types.Blob(data=audio_bytes, mime_type="audio/pcm;rate=16000")
        )

        # if stream gets paused, send:
        # await session.send_realtime_input(audio_stream_end=True)

        async for response in session.receive():
            if response.text is not None:
                print(response.text)

if __name__ == "__main__":
asyncio.run(main())
With send_realtime_input, the API will respond to audio automatically based on VAD. While send_client_content adds messages to the model context in order, send_realtime_input is optimized for responsiveness at the expense of deterministic ordering.

Configuring automatic VAD
For more control over the VAD activity, you can configure the following parameters. See API reference for more info.


from google.genai import types

config = {
"response_modalities": ["TEXT"],
"realtime_input_config": {
"automatic_activity_detection": {
"disabled": False, # default
"start_of_speech_sensitivity": types.StartSensitivity.START_SENSITIVITY_LOW,
"end_of_speech_sensitivity": types.EndSensitivity.END_SENSITIVITY_LOW,
"prefix_padding_ms": 20,
"silence_duration_ms": 100,
}
}
}
Disabling automatic VAD
Alternatively, the automatic VAD can be disabled by setting realtimeInputConfig.automaticActivityDetection.disabled to true in the setup message. In this configuration the client is responsible for detecting user speech and sending activityStart and activityEnd messages at the appropriate times. An audioStreamEnd isn't sent in this configuration. Instead, any interruption of the stream is marked by an activityEnd message.


config = {
"response_modalities": ["TEXT"],
"realtime_input_config": {"automatic_activity_detection": {"disabled": True}},
}

async with client.aio.live.connect(model=model, config=config) as session:
# ...
await session.send_realtime_input(activity_start=types.ActivityStart())
await session.send_realtime_input(
audio=types.Blob(data=audio_bytes, mime_type="audio/pcm;rate=16000")
)
await session.send_realtime_input(activity_end=types.ActivityEnd())
# ...
Token count
You can find the total number of consumed tokens in the usageMetadata field of the returned server message.


async for message in session.receive():
# The server will periodically send messages that include UsageMetadata.
if message.usage_metadata:
usage = message.usage_metadata
print(
f"Used {usage.total_token_count} tokens in total. Response token breakdown:"
)
for detail in usage.response_tokens_details:
match detail:
case types.ModalityTokenCount(modality=modality, token_count=count):
print(f"{modality}: {count}")
Extending the session duration
The maximum session duration can be extended to unlimited with two mechanisms:

Context window compression
Session resumption
Furthermore, you'll receive a GoAway message before the session ends, allowing you to take further actions.

Context window compression
To enable longer sessions, and avoid abrupt connection termination, you can enable context window compression by setting the contextWindowCompression field as part of the session configuration.

In the ContextWindowCompressionConfig, you can configure a sliding-window mechanism and the number of tokens that triggers compression.


from google.genai import types

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
context_window_compression=(
# Configures compression with default parameters.
types.ContextWindowCompressionConfig(
sliding_window=types.SlidingWindow(),
)
),
)
Session resumption
To prevent session termination when the server periodically resets the WebSocket connection, configure the sessionResumption field within the setup configuration.

Passing this configuration causes the server to send SessionResumptionUpdate messages, which can be used to resume the session by passing the last resumption token as the SessionResumptionConfig.handle of the subsequent connection.


import asyncio
from google import genai
from google.genai import types

client = genai.Client(api_key="GEMINI_API_KEY")
model = "gemini-2.0-flash-live-001"

async def main():
print(f"Connecting to the service with handle {previous_session_handle}...")
async with client.aio.live.connect(
model=model,
config=types.LiveConnectConfig(
response_modalities=["AUDIO"],
session_resumption=types.SessionResumptionConfig(
# The handle of the session to resume is passed here,
# or else None to start a new session.
handle=previous_session_handle
),
),
) as session:
while True:
await session.send_client_content(
turns=types.Content(
role="user", parts=[types.Part(text="Hello world!")]
)
)
async for message in session.receive():
# Periodically, the server will send update messages that may
# contain a handle for the current state of the session.
if message.session_resumption_update:
update = message.session_resumption_update
if update.resumable and update.new_handle:
# The handle should be retained and linked to the session.
return update.new_handle

                # For the purposes of this example, placeholder input is continually fed
                # to the model. In non-sample code, the model inputs would come from
                # the user.
                if message.server_content and message.server_content.turn_complete:
                    break

if __name__ == "__main__":
asyncio.run(main())
Receiving a message before the session disconnects
The server sends a GoAway message that signals that the current connection will soon be terminated. This message includes the timeLeft, indicating the remaining time and lets you take further action before the connection will be terminated as ABORTED.


async for response in session.receive():
if response.go_away is not None:
# The connection will soon be terminated
print(response.go_away.time_left)
Receiving a message when the generation is complete
The server sends a generationComplete message that signals that the model finished generating the response.


async for response in session.receive():
if response.server_content.generation_complete is True:
# The generation is complete
Media resolution
You can specify the media resolution for the input media by setting the mediaResolution field as part of the session configuration:


from google.genai import types

config = types.LiveConnectConfig(
response_modalities=["AUDIO"],
media_resolution=types.MediaResolution.MEDIA_RESOLUTION_LOW,
)
Limitations
Consider the following limitations of the Live API when you plan your project.

Response modalities
You can only set one response modality (TEXT or AUDIO) per session in the session configuration. Setting both results in a config error message. This means that you can configure the model to respond with either text or audio, but not both in the same session.

Client authentication
The Live API only provides server to server authentication and isn't recommended for direct client use. Client input should be routed through an intermediate application server for secure authentication with the Live API.

Session duration
Session duration can be extended to unlimited by enabling session compression. Without compression, audio-only sessions are limited to 15 minutes, and audio plus video sessions are limited to 2 minutes. Exceeding these limits without compression will terminate the connection.

Additionally, you can configure session resumption to allow the client to resume a session that was terminated.

Context window
A session has a context window limit of:

128k tokens for native audio output models
32k tokens for other Live API models

Live API - WebSockets API reference
Preview: The Live API is in preview.
The Live API is a stateful API that uses WebSockets. In this section, you'll find additional details regarding the WebSockets API.

Sessions
A WebSocket connection establishes a session between the client and the Gemini server. After a client initiates a new connection the session can exchange messages with the server to:

Send text, audio, or video to the Gemini server.
Receive audio, text, or function call requests from the Gemini server.
WebSocket connection
To start a session, connect to this websocket endpoint:


wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent
Note: The URL is for version v1beta.
Session configuration
The initial message after connection sets the session configuration, which includes the model, generation parameters, system instructions, and tools.

You can change the configuration parameters except the model during the session.

See the following example configuration. Note that the name casing in SDKs may vary. You can look up the Python SDK configuration options here.


{
"model": string,
"generationConfig": {
"candidateCount": integer,
"maxOutputTokens": integer,
"temperature": number,
"topP": number,
"topK": integer,
"presencePenalty": number,
"frequencyPenalty": number,
"responseModalities": [string],
"speechConfig": object,
"mediaResolution": object
},
"systemInstruction": string,
"tools": [object]
}
For more information on the API field, see generationConfig.

Send messages
To exchange messages over the WebSocket connection, the client must send a JSON object over an open WebSocket connection. The JSON object must have exactly one of the fields from the following object set:


{
"setup": BidiGenerateContentSetup,
"clientContent": BidiGenerateContentClientContent,
"realtimeInput": BidiGenerateContentRealtimeInput,
"toolResponse": BidiGenerateContentToolResponse
}
Supported client messages
See the supported client messages in the following table:

Message	Description
BidiGenerateContentSetup	Session configuration to be sent in the first message
BidiGenerateContentClientContent	Incremental content update of the current conversation delivered from the client
BidiGenerateContentRealtimeInput	Real time audio, video, or text input
BidiGenerateContentToolResponse	Response to a ToolCallMessage received from the server
Receive messages
To receive messages from Gemini, listen for the WebSocket 'message' event, and then parse the result according to the definition of the supported server messages.

See the following:


async with client.aio.live.connect(model='...', config=config) as session:
await session.send(input='Hello world!', end_of_turn=True)
async for message in session.receive():
print(message)
Server messages may have a usageMetadata field but will otherwise include exactly one of the other fields from the BidiGenerateContentServerMessage message. (The messageType union is not expressed in JSON so the field will appear at the top-level of the message.)

Messages and events
ActivityEnd
This type has no fields.

Marks the end of user activity.

ActivityHandling
The different ways of handling user activity.

Enums
ACTIVITY_HANDLING_UNSPECIFIED	If unspecified, the default behavior is START_OF_ACTIVITY_INTERRUPTS.
START_OF_ACTIVITY_INTERRUPTS	If true, start of activity will interrupt the model's response (also called "barge in"). The model's current response will be cut-off in the moment of the interruption. This is the default behavior.
NO_INTERRUPTION	The model's response will not be interrupted.
ActivityStart
This type has no fields.

Marks the start of user activity.

AudioTranscriptionConfig
This type has no fields.

The audio transcription configuration.

AutomaticActivityDetection
Configures automatic detection of activity.

Fields
disabled
bool

Optional. If enabled (the default), detected voice and text input count as activity. If disabled, the client must send activity signals.

startOfSpeechSensitivity
StartSensitivity

Optional. Determines how likely speech is to be detected.

prefixPaddingMs
int32

Optional. The required duration of detected speech before start-of-speech is committed. The lower this value, the more sensitive the start-of-speech detection is and shorter speech can be recognized. However, this also increases the probability of false positives.

endOfSpeechSensitivity
EndSensitivity

Optional. Determines how likely detected speech is ended.

silenceDurationMs
int32

Optional. The required duration of detected non-speech (e.g. silence) before end-of-speech is committed. The larger this value, the longer speech gaps can be without interrupting the user's activity but this will increase the model's latency.

BidiGenerateContentClientContent
Incremental update of the current conversation delivered from the client. All of the content here is unconditionally appended to the conversation history and used as part of the prompt to the model to generate content.

A message here will interrupt any current model generation.

Fields
turns[]
Content

Optional. The content appended to the current conversation with the model.

For single-turn queries, this is a single instance. For multi-turn queries, this is a repeated field that contains conversation history and the latest request.

turnComplete
bool

Optional. If true, indicates that the server content generation should start with the currently accumulated prompt. Otherwise, the server awaits additional messages before starting generation.

BidiGenerateContentRealtimeInput
User input that is sent in real time.

The different modalities (audio, video and text) are handled as concurrent streams. The ordering across these streams is not guaranteed.

This is different from BidiGenerateContentClientContent in a few ways:

Can be sent continuously without interruption to model generation.
If there is a need to mix data interleaved across the BidiGenerateContentClientContent and the BidiGenerateContentRealtimeInput, the server attempts to optimize for best response, but there are no guarantees.
End of turn is not explicitly specified, but is rather derived from user activity (for example, end of speech).
Even before the end of turn, the data is processed incrementally to optimize for a fast start of the response from the model.
Fields
mediaChunks[]
Blob

Optional. Inlined bytes data for media input. Multiple mediaChunks are not supported, all but the first will be ignored.

DEPRECATED: Use one of audio, video, or text instead.

audio
Blob

Optional. These form the realtime audio input stream.

video
Blob

Optional. These form the realtime video input stream.

activityStart
ActivityStart

Optional. Marks the start of user activity. This can only be sent if automatic (i.e. server-side) activity detection is disabled.

activityEnd
ActivityEnd

Optional. Marks the end of user activity. This can only be sent if automatic (i.e. server-side) activity detection is disabled.

audioStreamEnd
bool

Optional. Indicates that the audio stream has ended, e.g. because the microphone was turned off.

This should only be sent when automatic activity detection is enabled (which is the default).

The client can reopen the stream by sending an audio message.

text
string

Optional. These form the realtime text input stream.

BidiGenerateContentServerContent
Incremental server update generated by the model in response to client messages.

Content is generated as quickly as possible, and not in real time. Clients may choose to buffer and play it out in real time.

Fields
generationComplete
bool

Output only. If true, indicates that the model is done generating.

When model is interrupted while generating there will be no 'generation_complete' message in interrupted turn, it will go through 'interrupted > turn_complete'.

When model assumes realtime playback there will be delay between generation_complete and turn_complete that is caused by model waiting for playback to finish.

turnComplete
bool

Output only. If true, indicates that the model has completed its turn. Generation will only start in response to additional client messages.

interrupted
bool

Output only. If true, indicates that a client message has interrupted current model generation. If the client is playing out the content in real time, this is a good signal to stop and empty the current playback queue.

groundingMetadata
GroundingMetadata

Output only. Grounding metadata for the generated content.

inputTranscription
BidiGenerateContentTranscription

Output only. Input audio transcription. The transcription is sent independently of the other server messages and there is no guaranteed ordering.

outputTranscription
BidiGenerateContentTranscription

Output only. Output audio transcription. The transcription is sent independently of the other server messages and there is no guaranteed ordering, in particular not between serverContent and this outputTranscription.

urlContextMetadata
UrlContextMetadata

modelTurn
Content

Output only. The content that the model has generated as part of the current conversation with the user.

BidiGenerateContentServerMessage
Response message for the BidiGenerateContent call.

Fields
usageMetadata
UsageMetadata

Output only. Usage metadata about the response(s).

Union field messageType. The type of the message. messageType can be only one of the following:
setupComplete
BidiGenerateContentSetupComplete

Output only. Sent in response to a BidiGenerateContentSetup message from the client when setup is complete.

serverContent
BidiGenerateContentServerContent

Output only. Content generated by the model in response to client messages.

toolCall
BidiGenerateContentToolCall

Output only. Request for the client to execute the functionCalls and return the responses with the matching ids.

toolCallCancellation
BidiGenerateContentToolCallCancellation

Output only. Notification for the client that a previously issued ToolCallMessage with the specified ids should be cancelled.

goAway
GoAway

Output only. A notice that the server will soon disconnect.

sessionResumptionUpdate
SessionResumptionUpdate

Output only. Update of the session resumption state.

BidiGenerateContentSetup
Message to be sent in the first (and only in the first) BidiGenerateContentClientMessage. Contains configuration that will apply for the duration of the streaming RPC.

Clients should wait for a BidiGenerateContentSetupComplete message before sending any additional messages.

Fields
model
string

Required. The model's resource name. This serves as an ID for the Model to use.

Format: models/{model}

generationConfig
GenerationConfig

Optional. Generation config.

The following fields are not supported:

responseLogprobs
responseMimeType
logprobs
responseSchema
stopSequence
routingConfig
audioTimestamp
systemInstruction
Content

Optional. The user provided system instructions for the model.

Note: Only text should be used in parts and content in each part will be in a separate paragraph.

tools[]
Tool

Optional. A list of Tools the model may use to generate the next response.

A Tool is a piece of code that enables the system to interact with external systems to perform an action, or set of actions, outside of knowledge and scope of the model.

realtimeInputConfig
RealtimeInputConfig

Optional. Configures the handling of realtime input.

sessionResumption
SessionResumptionConfig

Optional. Configures session resumption mechanism.

If included, the server will send SessionResumptionUpdate messages.

contextWindowCompression
ContextWindowCompressionConfig

Optional. Configures a context window compression mechanism.

If included, the server will automatically reduce the size of the context when it exceeds the configured length.

inputAudioTranscription
AudioTranscriptionConfig

Optional. If set, enables transcription of voice input. The transcription aligns with the input audio language, if configured.

outputAudioTranscription
AudioTranscriptionConfig

Optional. If set, enables transcription of the model's audio output. The transcription aligns with the language code specified for the output audio, if configured.

proactivity
ProactivityConfig

Optional. Configures the proactivity of the model.

This allows the model to respond proactively to the input and to ignore irrelevant input.

BidiGenerateContentSetupComplete
This type has no fields.

Sent in response to a BidiGenerateContentSetup message from the client.

BidiGenerateContentToolCall
Request for the client to execute the functionCalls and return the responses with the matching ids.

Fields
functionCalls[]
FunctionCall

Output only. The function call to be executed.

BidiGenerateContentToolCallCancellation
Notification for the client that a previously issued ToolCallMessage with the specified ids should not have been executed and should be cancelled. If there were side-effects to those tool calls, clients may attempt to undo the tool calls. This message occurs only in cases where the clients interrupt server turns.

Fields
ids[]
string

Output only. The ids of the tool calls to be cancelled.

BidiGenerateContentToolResponse
Client generated response to a ToolCall received from the server. Individual FunctionResponse objects are matched to the respective FunctionCall objects by the id field.

Note that in the unary and server-streaming GenerateContent APIs function calling happens by exchanging the Content parts, while in the bidi GenerateContent APIs function calling happens over these dedicated set of messages.

Fields
functionResponses[]
FunctionResponse

Optional. The response to the function calls.

BidiGenerateContentTranscription
Transcription of audio (input or output).

Fields
text
string

Transcription text.

ContextWindowCompressionConfig
Enables context window compression — a mechanism for managing the model's context window so that it does not exceed a given length.

Fields
Union field compressionMechanism. The context window compression mechanism used. compressionMechanism can be only one of the following:
slidingWindow
SlidingWindow

A sliding-window mechanism.

triggerTokens
int64

The number of tokens (before running a turn) required to trigger a context window compression.

This can be used to balance quality against latency as shorter context windows may result in faster model responses. However, any compression operation will cause a temporary latency increase, so they should not be triggered frequently.

If not set, the default is 80% of the model's context window limit. This leaves 20% for the next user request/model response.

EndSensitivity
Determines how end of speech is detected.

Enums
END_SENSITIVITY_UNSPECIFIED	The default is END_SENSITIVITY_HIGH.
END_SENSITIVITY_HIGH	Automatic detection ends speech more often.
END_SENSITIVITY_LOW	Automatic detection ends speech less often.
GoAway
A notice that the server will soon disconnect.

Fields
timeLeft
Duration

The remaining time before the connection will be terminated as ABORTED.

This duration will never be less than a model-specific minimum, which will be specified together with the rate limits for the model.

ProactivityConfig
Config for proactivity features.

Fields
proactiveAudio
bool

Optional. If enabled, the model can reject responding to the last prompt. For example, this allows the model to ignore out of context speech or to stay silent if the user did not make a request, yet.

RealtimeInputConfig
Configures the realtime input behavior in BidiGenerateContent.

Fields
automaticActivityDetection
AutomaticActivityDetection

Optional. If not set, automatic activity detection is enabled by default. If automatic voice detection is disabled, the client must send activity signals.

activityHandling
ActivityHandling

Optional. Defines what effect activity has.

turnCoverage
TurnCoverage

Optional. Defines which input is included in the user's turn.

SessionResumptionConfig
Session resumption configuration.

This message is included in the session configuration as BidiGenerateContentSetup.sessionResumption. If configured, the server will send SessionResumptionUpdate messages.

Fields
handle
string

The handle of a previous session. If not present then a new session is created.

Session handles come from SessionResumptionUpdate.token values in previous connections.

SessionResumptionUpdate
Update of the session resumption state.

Only sent if BidiGenerateContentSetup.sessionResumption was set.

Fields
newHandle
string

New handle that represents a state that can be resumed. Empty if resumable=false.

resumable
bool

True if the current session can be resumed at this point.

Resumption is not possible at some points in the session. For example, when the model is executing function calls or generating. Resuming the session (using a previous session token) in such a state will result in some data loss. In these cases, newHandle will be empty and resumable will be false.

SlidingWindow
The SlidingWindow method operates by discarding content at the beginning of the context window. The resulting context will always begin at the start of a USER role turn. System instructions and any BidiGenerateContentSetup.prefixTurns will always remain at the beginning of the result.

Fields
targetTokens
int64

The target number of tokens to keep. The default value is trigger_tokens/2.

Discarding parts of the context window causes a temporary latency increase so this value should be calibrated to avoid frequent compression operations.

StartSensitivity
Determines how start of speech is detected.

Enums
START_SENSITIVITY_UNSPECIFIED	The default is START_SENSITIVITY_HIGH.
START_SENSITIVITY_HIGH	Automatic detection will detect the start of speech more often.
START_SENSITIVITY_LOW	Automatic detection will detect the start of speech less often.
TurnCoverage
Options about which input is included in the user's turn.

Enums
TURN_COVERAGE_UNSPECIFIED	If unspecified, the default behavior is TURN_INCLUDES_ONLY_ACTIVITY.
TURN_INCLUDES_ONLY_ACTIVITY	The users turn only includes activity since the last turn, excluding inactivity (e.g. silence on the audio stream). This is the default behavior.
TURN_INCLUDES_ALL_INPUT	The users turn includes all realtime input since the last turn, including inactivity (e.g. silence on the audio stream).
UrlContextMetadata
Metadata related to url context retrieval tool.

Fields
urlMetadata[]
UrlMetadata

List of url context.

UsageMetadata
Usage metadata about response(s).

Fields
promptTokenCount
int32

Output only. Number of tokens in the prompt. When cachedContent is set, this is still the total effective prompt size meaning this includes the number of tokens in the cached content.

cachedContentTokenCount
int32

Number of tokens in the cached part of the prompt (the cached content)

responseTokenCount
int32

Output only. Total number of tokens across all the generated response candidates.

toolUsePromptTokenCount
int32

Output only. Number of tokens present in tool-use prompt(s).

thoughtsTokenCount
int32

Output only. Number of tokens of thoughts for thinking models.

totalTokenCount
int32

Output only. Total token count for the generation request (prompt + response candidates).

promptTokensDetails[]
ModalityTokenCount

Output only. List of modalities that were processed in the request input.

cacheTokensDetails[]
ModalityTokenCount

Output only. List of modalities of the cached content in the request input.

responseTokensDetails[]
ModalityTokenCount

Output only. List of modalities that were returned in the response.

toolUsePromptTokensDetails[]
ModalityTokenCount

Output only. List of modalities that were processed for tool-use request inputs.

Ephemeral authentication tokens
Ephemeral authentication tokens can be obtained by calling AuthTokenService.CreateToken and then used with GenerativeService.BidiGenerateContentConstrained, either by passing the token in an access_token query parameter, or in an HTTP Authorization header with "Token" prefixed to it.

CreateAuthTokenRequest
Create an ephemeral authentication token.

Fields
authToken
AuthToken

Required. The token to create.

AuthToken
A request to create an ephemeral authentication token.

Fields
name
string

Output only. Identifier. The token itself.

expireTime
Timestamp

Optional. Input only. Immutable. An optional time after which, when using the resulting token, messages in BidiGenerateContent sessions will be rejected. (Gemini may preemptively close the session after this time.)

If not set then this defaults to 30 minutes in the future. If set, this value must be less than 20 hours in the future.

newSessionExpireTime
Timestamp

Optional. Input only. Immutable. The time after which new Live API sessions using the token resulting from this request will be rejected.

If not set this defaults to 60 seconds in the future. If set, this value must be less than 20 hours in the future.

fieldMask
FieldMask

Optional. Input only. Immutable. If field_mask is empty, and bidiGenerateContentSetup is not present, then the effective BidiGenerateContentSetup message is taken from the Live API connection.

If field_mask is empty, and bidiGenerateContentSetup is present, then the effective BidiGenerateContentSetup message is taken entirely from bidiGenerateContentSetup in this request. The setup message from the Live API connection is ignored.

If field_mask is not empty, then the corresponding fields from bidiGenerateContentSetup will overwrite the fields from the setup message in the Live API connection.

Union field config. The method-specific configuration for the resulting token. config can be only one of the following:
bidiGenerateContentSetup
BidiGenerateContentSetup

Optional. Input only. Immutable. Configuration specific to BidiGenerateContent.

uses
int32

Optional. Input only. Immutable. The number of times the token can be used. If this value is zero then no limit is applied. Resuming a Live API session does not count as a use. If unspecified, the default is 1.

More information on common types

For more information on the commonly-used API resource types Blob, Content, FunctionCall, FunctionResponse, GenerationConfig, GroundingMetadata, ModalityTokenCount, and Tool, see Generating content.

