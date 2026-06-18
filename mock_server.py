"""Standalone mock OpenAI-compatible API server.

Returns the last user message content as the assistant response.
Useful for development when no local LLM is available.

Usage:
    uv run python mock_server.py [--port 8001]
"""

import argparse
import logging

import uvicorn
from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI(title="mock-openai-server")
_LOGGER = logging.getLogger(__name__)


class Message(BaseModel):
    role: str
    content: str


class ChatCompletionRequest(BaseModel):
    model: str = "mock-model"
    messages: list[Message]
    temperature: float | None = None
    max_tokens: int | None = None


class Choice(BaseModel):
    index: int = 0
    message: Message
    finish_reason: str = "stop"


class ChatCompletionResponse(BaseModel):
    id: str = "mock-completion"
    object: str = "chat.completion"
    model: str = "mock-model"
    choices: list[Choice]


@app.post("/v1/chat/completions")
async def chat_completions(req: ChatCompletionRequest) -> ChatCompletionResponse:
    user_msgs = [m for m in req.messages if m.role == "user"]
    last_user = user_msgs[-1].content if user_msgs else ""
    _LOGGER.info("Mock returning %d chars", len(last_user))
    return ChatCompletionResponse(
        choices=[Choice(message=Message(role="assistant", content=last_user))]
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Mock OpenAI-compatible API server")
    parser.add_argument("--port", type=int, default=8001, help="Port to listen on")
    args = parser.parse_args()

    logging.basicConfig(
        level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s"
    )
    _LOGGER.info("Starting mock server on port %d", args.port)
    uvicorn.run(app, host="127.0.0.1", port=args.port)


if __name__ == "__main__":
    main()
