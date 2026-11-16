"""The same framing cases as dvm-api's frame.rs."""
import pytest

from dvm.frame import (
    MAX_HEADER, MAX_PAYLOAD, BadHeader, Cut, Frame, FrameDecoder,
    LongHeader, LongPayload, OutputAfter, Trailing, decode_frame, decode_output, encode_frame,
)


def take(decoder):
    frames = []
    while (frame := decoder.next_frame()) is not None:
        frames.append(frame)
    return frames


def test_frames_carry_raw_bytes_and_split_wherever_the_stream_is_cut():
    payload = b'\n{"id":9}\n\xff'
    stream = encode_frame({"id": 1}, payload) + encode_frame({"id": 2})
    assert stream[:23] == b'{"id":1,"bytes":11}\n\n{"'
    expected = [Frame({"id": 1}, payload, range(31)), Frame({"id": 2}, b"", range(31, 40))]
    for cut in range(len(stream) + 1):
        decoder = FrameDecoder()
        decoder.push(stream[:cut])
        frames = take(decoder)
        decoder.push(stream[cut:])
        assert frames + take(decoder) == expected
        assert decoder.buffered() == 0


def test_a_bad_header_costs_its_frame_and_a_long_one_ends_the_stream():
    decoder = FrameDecoder()
    decoder.push(b'not json\n[1]\n{"bytes":-1}\n{"bytes":1.0}\n{"bytes":true}\n{"id":4}\n')
    for _ in range(5):
        with pytest.raises(BadHeader):
            decoder.next_frame()
    assert decoder.next_frame().header == {"id": 4}

    decoder.push(f'{{"id":5,"bytes":{MAX_PAYLOAD + 1}}}\n'.encode())
    with pytest.raises(LongPayload):
        decoder.next_frame()
    decoder.push(b'{"id":6}\n')
    with pytest.raises(LongPayload):
        decoder.next_frame()

    decoder = FrameDecoder()
    decoder.push(b" " * MAX_HEADER)
    assert decoder.next_frame() is None
    decoder.push(b" ")
    with pytest.raises(LongHeader):
        decoder.next_frame()


def test_a_whole_frame_is_one_frame_and_nothing_more():
    frame = encode_frame({"id": 1}, b"hi")
    assert decode_frame(frame).payload == b"hi"
    with pytest.raises(Cut):
        decode_frame(frame[:-1])
    with pytest.raises(Trailing) as error:
        decode_frame(frame + frame)
    assert error.value.length == len(frame)
    assert encode_frame({}, b"x") == b'{"bytes":1}\nx'


def test_output_counts_match_the_payload():
    frame = encode_frame({"stdout": 2, "stderr": 3, "more": False, "cut": True}, b"\0\xfferr")
    assert decode_output(frame) == OutputAfter(b"\0\xff", b"err", False, True)
    for header in ({"stdout": 9, "stderr": 0, "more": True, "cut": False},
                   {"stdout": 2, "stderr": 0, "more": 1, "cut": False},
                   {"stdout": 2, "stderr": 0, "more": True}):
        with pytest.raises(BadHeader):
            decode_output(encode_frame(header, b"hi"))
