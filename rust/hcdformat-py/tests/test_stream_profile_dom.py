import ast
import os

import pytest

import hcdf
from hcdf.dom import (
    StreamProfileDocument,
    dump_stream_profile,
    dumps_stream_profile,
    load_stream_profile,
    loads_stream_profile,
)


REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
FIXTURE = os.path.join(
    REPO_ROOT, "rust", "hcdformat-rs", "tests", "fixtures", "typed-stream-profile.xml"
)


def _source():
    with open(FIXTURE, "r", encoding="utf-8") as source:
        return source.read().removesuffix("\n")


def test_real_stream_profile_round_trip_and_nested_navigation():
    source = _source()
    document = loads_stream_profile(source)

    assert isinstance(document, StreamProfileDocument)
    assert document.name == "operational"
    assert document.version == "1.0"
    assert document.description == "Operational traffic"
    assert len(document.dependency) == 2
    assert document.dependency[0].required is True
    assert document.dependency[1].required is False
    assert len(document.stream_group) == 1
    assert len(document.stream) == 2

    stream = document.stream[0]
    assert stream.protocol == "ieee:1722"
    assert stream.group_ref.group == "arm-control"
    assert stream.group_ref.instance.segment[0].name == "arm-module"
    assert stream.group_ref.instance.segment[0].occurrence == 1
    assert len(stream.path.network) == 2
    assert stream.path.network[0].instance.segment[0].occurrence == 1
    assert len(stream.path.forwarding) == 1
    forwarding = stream.path.forwarding[0]
    assert forwarding.from_.participant.participant == "gateway-arm"
    assert forwarding.function.function == "arm-bridge"
    assert forwarding.to.participant.participant == "gateway-backbone"
    assert stream.talker.participant.participant == "controller"
    assert len(stream.listener) == 2
    assert stream.traffic_class_ref.traffic_class == "control"
    assert stream.schedule_ref.schedule == "fast-cycle"
    assert stream.frer.seamless_trees == 2
    assert stream.frer.sequence_encoding == "r-tag"

    assert dumps_stream_profile(document) == source
    assert StreamProfileDocument.loads(source).dumps() == source


def test_stream_profile_edits_are_rust_validated_and_lossless():
    document = loads_stream_profile(_source().encode("utf-8"))
    stream = document.stream[0]

    stream.protocol = "vendor:command-v2"
    stream.group_ref.instance.segment[0].occurrence = 3
    stream.path.network[0].network = "left-arm-updated"
    stream.talker.participant.participant = "controller-updated"
    stream.listener[0].participant.participant = "joint-updated"
    stream.frer.sequence_encoding = "hsr"

    with pytest.raises(ValueError, match="qualified identifier"):
        stream.protocol = "unqualified"

    serialized = dumps_stream_profile(document)
    reparsed = loads_stream_profile(serialized)
    reparsed_stream = reparsed.stream[0]
    assert reparsed_stream.protocol == "vendor:command-v2"
    assert reparsed_stream.group_ref.instance.segment[0].occurrence == 3
    assert reparsed_stream.path.network[0].network == "left-arm-updated"
    assert reparsed_stream.talker.participant.participant == "controller-updated"
    assert reparsed_stream.listener[0].participant.participant == "joint-updated"
    assert reparsed_stream.frer.sequence_encoding == "hsr"

    reparsed_stream.protocol = None
    assert reparsed_stream.protocol is None
    assert dumps_stream_profile(reparsed).count(" protocol=") == 1


def test_post_load_scalar_mutation_cannot_bypass_serializer_validation():
    document = loads_stream_profile(_source())
    stream = document.stream[0]

    stream.pcp = 8
    with pytest.raises(ValueError, match="pcp must be in 0..=7"):
        dumps_stream_profile(document)
    with pytest.raises(ValueError, match="pcp must be in 0..=7"):
        document.dumps()

    stream.pcp = 5
    stream.max_frame_size_bytes = 0
    with pytest.raises(ValueError, match="max-frame-size-bytes must be greater than zero"):
        dumps_stream_profile(document)
    with pytest.raises(ValueError, match="max-frame-size-bytes must be greater than zero"):
        document.dumps()


def test_stream_profile_file_functions_use_the_same_rust_dom(tmp_path):
    source_path = tmp_path / "input.streams.xml"
    output_path = tmp_path / "output.streams.xml"
    source_path.write_text(_source(), encoding="utf-8")

    document = load_stream_profile(str(source_path))
    dump_stream_profile(document, str(output_path))
    assert output_path.read_text(encoding="utf-8") == _source()
    assert StreamProfileDocument.load(str(output_path)).dumps() == _source()


def test_stream_profile_classes_and_stub_are_complete():
    expected = [
        "PyStreamProfileDocument",
        "PyStreamProfileDependency",
        "PyStreamGroup",
        "PyStreamGroupRef",
        "PyStreamDefinition",
        "PyStreamPath",
        "PyStreamForwarding",
        "PyStreamForwardingEnd",
        "PyStreamTalker",
        "PyStreamListener",
        "PyFrer",
        "PyStreamProfileInstanceRef",
        "PyStreamProfileInstanceSegment",
        "PyStreamProfileNetworkRef",
        "PyStreamProfileParticipantRef",
        "PyStreamProfileConnectivityFunctionRef",
        "PyStreamProfileTrafficClassRef",
        "PyStreamProfileScheduleRef",
    ]
    stub = hcdf.dom_pyi()
    ast.parse(stub)
    for name in expected:
        assert hasattr(hcdf, name), name
        assert f"class {name}:" in stub, name

    assert "StreamProfileDocument = PyStreamProfileDocument" in stub
    assert "def loads_stream_profile(" in stub
    assert "def dumps_stream_profile(" in stub
    assert "protocol: Optional[str]" in stub
    assert "ChainRef" not in stub
