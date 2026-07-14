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
    ]
    for args, expected in cases:
        actual = ignorable_request_failure(*args)
        if actual != expected:
            raise AssertionError(f"{args}: expected {expected}, got {actual}")
    print(f"passed {len(cases)} browser request-failure classifier cases")


if __name__ == "__main__":
    main()
