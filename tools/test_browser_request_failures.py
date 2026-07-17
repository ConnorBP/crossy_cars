#!/usr/bin/env python3
import json

from browser_scenarios import (
    fixture_capability_admitted,
    ignorable_request_failure,
    is_v3_write,
)


def main() -> None:
    cases = [
        (("POST", "https://car.segfault.site/cdn-cgi/rum?", "net::ERR_ABORTED"), True),
        (("POST", "https://car.segfault.site/cdn-cgi/rum", "net::ERR_FAILED"), False),
        (("GET", "https://car.segfault.site/cdn-cgi/rum", "net::ERR_ABORTED"), False),
        (("POST", "https://car.segfault.site/cdn-cgi/rum/other", "net::ERR_ABORTED"), False),
        (("POST", "https://car.segfault.site/other?next=/cdn-cgi/rum", "net::ERR_ABORTED"), False),
        (("POST", "https://car.segfault.site/v1/leaderboard?limit=10", "net::ERR_ABORTED"), True),
        (("GET", "https://car.segfault.site/api/game", "net::ERR_ABORTED"), False),
        (("POST", "http://[invalid/cdn-cgi/rum", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_ABORTED"), True),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit", "net::ERR_ABORTED"), True),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/b/3104729c556c/api.js", "net::ERR_ABORTED"), True),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/b/3104729c556c/api.js?x=1", "net::ERR_ABORTED"), True),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_FAILED"), False),
        (("POST", "https://challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "http://challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com:444/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com:bad/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://user@challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://evil.challenges.cloudflare.com/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com.evil.test/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/b/UPPER/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/b//api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/x/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/api.js/more", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/turnstile/v0/other.js", "net::ERR_ABORTED"), False),
        (("GET", "https://challenges.cloudflare.com/other?next=/turnstile/v0/api.js", "net::ERR_ABORTED"), False),
    ]
    for args, expected in cases:
        actual = ignorable_request_failure(*args)
        if actual != expected:
            raise AssertionError(f"{args}: expected {expected}, got {actual}")

    v3_cases = [
        (("POST", "https://example.test/v3/session"), True),
        (("POST", "https://example.test/v3/scores?retry=0"), True),
        (("POST", "https://example.test/v3/evidence"), True),
        (("GET", "https://example.test/v3/capabilities"), False),
        (("POST", "https://example.test/v1/leaderboard"), False),
        (("POST", "https://example.test/other?next=/v3/scores"), False),
        (("POST", "http://[invalid/v3/scores"), False),
    ]
    for args, expected in v3_cases:
        actual = is_v3_write(*args)
        if actual != expected:
            raise AssertionError(f"{args}: expected v3 write {expected}, got {actual}")

    exact = {
        "ranked": {
            "enabled": True,
            "categories": [
                "rotation.v2.cluck_hunt",
                "rotation.v2.right_of_way",
            ],
        },
        "protocolVersion": 3,
        "protocolId": "roady-protocol.v3",
        "rulesVersion": 3,
        "rulesId": "roady-rules.v3",
        "policyVersion": 1,
        "policyId": "roady-ranked-policy.v3.1",
        "mode": "rotation",
    }
    capability_cases = [(exact, True)]
    for mutation in (
        {**exact, "protocolVersion": 2},
        {**exact, "extra": 1},
        {**exact, "ranked": {**exact["ranked"], "enabled": False}},
        {**exact, "ranked": {**exact["ranked"], "categories": list(reversed(exact["ranked"]["categories"]))}},
        {**exact, "ranked": {**exact["ranked"], "categories": exact["ranked"]["categories"][:1]}},
    ):
        capability_cases.append((mutation, False))
    for fixture, expected in capability_cases:
        actual = fixture_capability_admitted(json.dumps(fixture))
        if actual != expected:
            raise AssertionError(f"capability fixture expected {expected}, got {actual}: {fixture}")
    print(
        f"passed {len(cases)} failure, {len(v3_cases)} v3-write, "
        f"and {len(capability_cases)} capability-gate cases"
    )


if __name__ == "__main__":
    main()
