# https://gitlab.com/kicad/code/kicad-python
# pip install kicad-python

from kipy import KiCad, errors
from kipy.board import Board
from kipy.board_types import (
    ArcTrack,
    FootprintInstance,
    Net,
    Pad,
    Track,
    Via,
    Zone,
    Field,
    PadStack,
)
from kipy.proto.common.types.enums_pb2 import KiCadObjectType


def connect_kicad():
    try:
        kicad = KiCad()
        kicad.get_version()
        return kicad
    except BaseException as e:
        print(f"Not connected to KiCad: {e}")
        return None


def print_ArcTrack(obj: ArcTrack):
    print("id                ", obj.id.value)  # KIID
    print("net               ", obj.net)  # Net
    print("layer             ", obj.layer)  # BoardLayer.ValueType
    print("start             ", obj.start)  # Vector2
    print("end               ", obj.end)  # Vector2
    print("width             ", obj.width)  # int
    print("mid               ", obj.mid)  # Vector2


def print_Footprint(obj: FootprintInstance):
    print("id                ", obj.id.value)  # KIID
    print("position          ", obj.position)  # Vector2:
    print("orientation       ", obj.orientation)  # Angle:
    print("layer             ", obj.layer)  # BoardLayer.ValueType:
    print("locked            ", obj.locked)  # bool:
    print("definition        ", obj.definition)  # Footprint:
    print("reference_field   ", obj.reference_field.text.value)  # Field:
    print("value_field       ", obj.value_field.text.value)  # Field:
    print("datasheet_field   ", obj.datasheet_field.text.value)  # Field:
    print("description_field ", obj.description_field.text.value)  # Field:
    print("attributes        ", obj.attributes)  # FootprintAttributes:
    # print("texts_and_fields  ", obj.texts_and_fields)
    for item in obj.texts_and_fields:
        if type(item) == Field and len(item.text.value):
            print(item.name, "-->", item.text.value)


def print_Net(obj: Net):
    print("name              ", obj.name)  # str
    print("code              ", obj.code)  # int


def print_Pad(obj: Pad):
    print("id                ", obj.id.value)  # KIID
    print("number            ", obj.number)  # str
    print("position          ", obj.position)  # Vector2
    print("net               ", obj.net)  # Net
    print("pad_type          ", obj.pad_type)  # PadType.ValueType
    # print("padstack          ", obj.padstack)  # PadStack
    print_PadStack(obj.padstack)


def print_Track(obj: Track):
    print("id                ", obj.id.value)  # KIID
    print("net               ", obj.net)  #  Net
    print("layer             ", obj.layer)  #  BoardLayer.ValueType
    print("start             ", obj.start)  #  Vector2
    print("end               ", obj.end)  #  Vector2
    print("width             ", obj.width)  #  int


def print_PadStack(obj: PadStack):
    print("type                     ", obj.type)
    # board_types_pb2.PadStackType.ValueType:
    print("layers                   ", obj.layers)  # Sequence[BoardLayer.ValueType]:
    print("drill                    ", obj.drill)  # DrillProperties:
    print("unconnected_layer_removal", obj.unconnected_layer_removal)
    # UnconnectedLayerRemoval.ValueType:
    print("copper_layers            ", obj.copper_layers)  # list[PadStackLayer]:
    print("angle                    ", obj.angle)  # Angle:
    print("front_outer_layers       ", obj.front_outer_layers)  # PadStackOuterLayer:
    print("back_outer_layers        ", obj.back_outer_layers)  # PadStackOuterLayer:
    print("zone_settings            ", obj.zone_settings)  # ZoneConnectionSettings:


def print_Via(obj: Via):
    print("id                ", obj.id.value)  # KIID
    print("position          ", obj.position)  # Vector2
    print("net               ", obj.net)  # Net
    print("locked            ", obj.locked)  # bool
    print("type              ", obj.type)  # ViaType.ValueType
    # print("padstack          ", obj.padstack)  # PadStack
    print_PadStack(obj.padstack)
    print("diameter          ", obj.diameter)  # int
    print("drill_diameter    ", obj.drill_diameter)  # int


def print_Zone(obj: Zone):
    print("id                ", obj.id.value)  # KIID
    print("type              ", obj.type)  # ZoneType.ValueType
    print("layers            ", obj.layers)  # Sequence[BoardLayer.ValueType]
    print("outline           ", obj.outline)  # PolygonWithHoles
    print("name              ", obj.name)  # str
    print("priority          ", obj.priority)  # int
    print("filled            ", obj.filled)  # bool
    print("locked            ", obj.locked)  # bool
    print("filled_polygons   ", obj.filled_polygons)
    # dict[BoardLayer.ValueType, list[PolygonWithHoles]]
    print("connection        ", obj.connection)  # Optional[ZoneConnectionSettings]
    print("clearance         ", obj.clearance)  # Optional[int]
    print("min_thickness     ", obj.min_thickness)  # Optional[int]
    print("island_mode       ", obj.island_mode)
    # Optional[IslandRemovalMode.ValueType]
    print("min_island_area   ", obj.min_island_area)  # Optional[int]
    print("fill_mode         ", obj.fill_mode)  # Optional[ZoneFillMode.ValueType]
    print("net               ", obj.net)  # Optional[Net]
    print("teardrop          ", obj.teardrop)
    # Optional[board_types_pb2.TeardropSettings]
    print("border_style      ", obj.border_style)  # ZoneBorderStyle.ValueType
    print("border_hatch_pitch", obj.border_hatch_pitch)  # int


def main(kicad: KiCad):
    try:
        board: Board = kicad.get_board()
    except errors.ApiError as e:
        print("Fehler: Kein PCB geöffnet.")
        return

    try:
        stackup = board.get_stackup()
        print("Stackup PCB:")
        for layer in stackup.layers:
            print(f"Name: {layer.user_name}, Dicke: {layer.thickness/1000:0.0f} um")
    except:
        print("Error load Stackup")

    select = board.get_selection(types=(KiCadObjectType.KOT_PCB_FOOTPRINT,))
    for object in select:
        if type(object) == FootprintInstance:
            print_Footprint(object)
        else:
            print("ERROR", type(object))

    footprints = board.get_footprints()
    for footprint in footprints:
        print_Footprint(footprint)

    nets = board.get_nets()
    for net in nets:
        print_Net(net)

    pads = board.get_pads()
    for pad in pads:
        print_Pad(pad)

    tracks = board.get_tracks()
    for track in tracks:
        if type(track) == Track:
            print_Track(track)
        elif type(track) == ArcTrack:
            print_ArcTrack(track)

    vias = board.get_vias()
    for via in vias:
        print_Via(via)

    zones = board.get_zones()
    for zone in zones:
        print_Zone(zone)


if __name__ == "__main__":
    kicad = connect_kicad()

    if kicad:
        main(kicad)
