import logging

from httpx import AsyncClient, HTTPStatusError, RequestError

_LOGGER = logging.getLogger(__name__)


class LLMClient:
    """Async OpenAI-compatible API client."""

    def __init__(self, base_url: str, api_key: str, model: str, timeout: int = 120) -> None:
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.model = model
        self.timeout = timeout

    async def translate(
        self,
        system_prompt: str,
        user_text: str,
        max_tokens: int | None = None,
    ) -> str:
        """Send a translation request and return the assistant response."""
        if not user_text.strip():
            msg = "translate() called with empty user_text"
            raise ValueError(msg)
        payload: dict[str, object] = {
            "model": self.model,
            "messages": [],
        }
        if system_prompt.strip():
            payload["messages"].append({"role": "system", "content": system_prompt})  # type: ignore[union-attr]
        payload["messages"].append({"role": "user", "content": user_text})  # type: ignore[union-attr]
        if max_tokens is not None:
            payload["max_tokens"] = max_tokens

        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }

        url = f"{self.base_url}/chat/completions"

        _LOGGER.debug("LLM request: %s", {k: v for k, v in payload.items() if k != "messages"})
        for msg in payload["messages"]:  # type: ignore[union-attr]
            content = str(msg.get("content", ""))  # type: ignore[union-attr]
            _LOGGER.debug("  [%s] len=%d: %r", msg.get("role"), len(content), content[:200])  # type: ignore[union-attr]

        async with AsyncClient(timeout=self.timeout) as client:
            try:
                resp = await client.post(url, json=payload, headers=headers)
                resp.raise_for_status()
                data = resp.json()
                _LOGGER.debug("LLM response: %r", data)
            except HTTPStatusError as e:
                _LOGGER.exception("API returned %s: %s", e.response.status_code, e.response.text)
                raise
            except RequestError:
                _LOGGER.exception("Request failed")
                raise

        choices = data.get("choices", [])
        if not choices:
            msg = "API response missing choices"
            raise ValueError(msg)

        return choices[0].get("message", {}).get("content", "")
