from __future__ import annotations

from local_llm_translator.llm import LLMClient


class TestLLMClient:
    async def test_translate_success(self, httpx_mock):
        httpx_mock.add_response(
            url="http://test/v1/chat/completions",
            method="POST",
            json={
                "choices": [{"message": {"content": "Hello translated"}}],
            },
        )
        client = LLMClient(
            base_url="http://test/v1",
            api_key="sk-test",
            model="test-model",
            timeout=5,
        )
        result = await client.translate(
            system_prompt="Translate to Italian",
            user_text="Hello",
        )
        assert result == "Hello translated"

    async def test_translate_empty_response(self, httpx_mock):
        httpx_mock.add_response(
            url="http://test/v1/chat/completions",
            method="POST",
            json={"choices": []},
        )
        client = LLMClient(
            base_url="http://test/v1",
            api_key="sk-test",
            model="test-model",
            timeout=5,
        )
        import pytest

        with pytest.raises(ValueError, match="missing choices"):
            await client.translate("system", "user text")

    async def test_translate_http_error(self, httpx_mock):
        httpx_mock.add_response(
            url="http://test/v1/chat/completions",
            method="POST",
            status_code=401,
            json={"error": "unauthorized"},
        )
        client = LLMClient(
            base_url="http://test/v1",
            api_key="bad-key",
            model="test-model",
            timeout=5,
        )
        import pytest
        from httpx import HTTPStatusError

        with pytest.raises(HTTPStatusError):
            await client.translate("system", "user text")

    async def test_request_payload(self, httpx_mock):
        httpx_mock.add_response(
            url="http://test/v1/chat/completions",
            method="POST",
            json={"choices": [{"message": {"content": "ok"}}]},
        )
        client = LLMClient("http://test/v1", "sk-test", "llama3", timeout=5)
        await client.translate("Be a translator", "Hello world")

        import json

        request = httpx_mock.get_request()
        body = json.loads(request.content)
        assert body["model"] == "llama3"
        assert len(body["messages"]) == 2
        assert body["messages"][0]["role"] == "system"
        assert body["messages"][0]["content"] == "Be a translator"
        assert body["messages"][1]["role"] == "user"
        assert body["messages"][1]["content"] == "Hello world"

    async def test_includes_auth_header(self, httpx_mock):
        httpx_mock.add_response(
            url="http://test/v1/chat/completions",
            method="POST",
            json={"choices": [{"message": {"content": "ok"}}]},
        )
        client = LLMClient("http://test/v1", "my-secret-key", "model", timeout=5)
        await client.translate("sys", "user")

        request = httpx_mock.get_request()
        assert request.headers["authorization"] == "Bearer my-secret-key"
