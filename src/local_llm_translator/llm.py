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
    ) -> str:
        """Send a translation request and return the assistant response."""
        payload = {
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_text},
            ],
        }

        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }

        url = f"{self.base_url}/chat/completions"

        async with AsyncClient(timeout=self.timeout) as client:
            try:
                resp = await client.post(url, json=payload, headers=headers)
                resp.raise_for_status()
                data = resp.json()
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
