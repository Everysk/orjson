# SPDX-License-Identifier: (Apache-2.0 OR MIT)


import orjson


class TestLoadNanAsNoneTests:
    def test_nan_inf_in_object(self):
        """
        Test that NaN, Infinity, and -Infinity are loaded as None in dict values with OPT_NAN_AS_NULL.
        """
        data = b'{"a": nan, "b": Infinity, "c": -Infinity, "d": NaN, "e": 2}'
        result = orjson.loads(data, option=orjson.OPT_NAN_AS_NULL)
        assert result == {"a": None, "b": None, "c": None, "d": None, "e": 2}

    def test_nan_inf_nested_list(self):
        """
        Test that NaN, Infinity, and -Infinity are loaded as None in nested lists with OPT_NAN_AS_NULL.
        """
        data = b"[1, [nan, 2, [Infinity, -Infinity, NaN]], 3]"
        result = orjson.loads(data, option=orjson.OPT_NAN_AS_NULL)
        assert result == [1, [None, 2, [None, None, None]], 3]

    def test_nan_inf_nested_object(self):
        """
        Test that NaN, Infinity, and -Infinity are loaded as None in nested dicts with OPT_NAN_AS_NULL.
        """
        data = b'{"x": {"y": nan, "z": [Infinity, -Infinity, NaN]}, "w": 5}'
        result = orjson.loads(data, option=orjson.OPT_NAN_AS_NULL)
        assert result == {"x": {"y": None, "z": [None, None, None]}, "w": 5}

    def test_nan_inf_mixed(self):
        """
        Test that NaN, Infinity, and -Infinity are loaded as None in mixed structures with OPT_NAN_AS_NULL.
        """
        data = b'[{"a": nan}, [Infinity, {"b": -Infinity}], NaN]'
        result = orjson.loads(data, option=orjson.OPT_NAN_AS_NULL)
        assert result == [{"a": None}, [None, {"b": None}], None]

    def test_nan_inf_as_none_flag_combinations(self):
        """
        Test that NaN, Infinity, and -Infinity are loaded as None with OPT_NAN_AS_NULL and other flags.
        """
        options = [
            orjson.OPT_NAN_AS_NULL | orjson.OPT_BIG_INTEGER,
        ]

        test_cases = [
            (b"NaN", None),
            (b"Infinity", None),
            (b"-Infinity", None),
            (
                b'{"nan": NaN, "inf": Infinity, "neginf": -Infinity}',
                {"nan": None, "inf": None, "neginf": None},
            ),
            (b'{"a": 1, "b": "test", "c": true}', {"a": 1, "b": "test", "c": True}),
            (b'{"b": 2, "a": 1}', {"b": 2, "a": 1}),
            (b"123", 123),
            (b"-123", -123),
            (b"100000000000000000001", 100000000000000000001),
            (b"-100000000000000000001", -100000000000000000001),
            (
                b'{"big": 123456789012345678901234567890}',
                {"big": 123456789012345678901234567890},
            ),
        ]
        for option in options:
            print(f"Testing with option: {option}")
            for data, expected in test_cases:
                result = orjson.loads(data, option=option)
                assert result == expected
