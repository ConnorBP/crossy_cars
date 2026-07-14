#!/usr/bin/env python3
from browser_scenarios import ignorable_request_failure


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
    print(f"passed {len(cases)} browser request-failure classifier cases")


if __name__ == "__main__":
    main()
