#!/usr/bin/env python3
"""Deterministic fail-closed Ranked-v3 capability fixture for local/CI browser QA."""
from __future__ import annotations

import json
from typing import Any

DISABLED_CAPABILITY = {
    "ranked": {
        "enabled": False,
        "categories": ["rotation.v2.cluck_hunt", "rotation.v2.right_of_way"],
    },
    "protocolVersion": 3,
    "protocolId": "roady-protocol.v3",
    "rulesVersion": 3,
    "rulesId": "roady-rules.v3",
    "policyVersion": 1,
    "policyId": "roady-ranked-policy.v3.1",
    "mode": "rotation",
}


def install_disabled_capability(context: Any) -> None:
    """Fulfill only /v3/capabilities; every write remains observable/fail-closed."""
    body = json.dumps(DISABLED_CAPABILITY, separators=(",", ":"))

    def route_handler(route: Any) -> None:
        if route.request.url.split("?", 1)[0].endswith("/v3/capabilities"):
            route.fulfill(
                status=200,
                content_type="application/json",
                headers={"Cache-Control": "no-store"},
                body=body,
            )
        else:
            route.continue_()

    context.route("**/*", route_handler)
