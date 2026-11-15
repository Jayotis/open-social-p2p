import 'package:flutter_test/flutter_test.dart';
import 'package:osp_app/search_lights.dart';
import 'package:osp_app/src/rust/api/osp.dart';

FoundPerson found(String name, List<String> regions) => FoundPerson(
  identity: 'id-$name',
  name: name,
  followed: false,
  regions: regions,
);

UniverseGroup group(int id) => UniverseGroup(
  id: BigInt.from(id),
  people: 3,
  bright: 0,
  lit: 0,
  x: 0,
  y: 0,
);

SkyCell cell(int level, int x, int y) => SkyCell(
  level: level,
  x: x,
  y: y,
  people: 4,
  creators: 0,
  lit: 0,
  atX: 0,
  atY: 0,
);

SearchLights lights(List<FoundPerson> people) =>
    SearchLights('w', Found(people: people, posts: const []));

void main() {
  test('flies to the deepest group holding every result', () {
    expect(
      lights([
        found('Kim', ['', 'g:1', 'g:4']),
        found('Sam', ['', 'g:1', 'g:5']),
      ]).region,
      'g:1',
    );
    expect(
      lights([
        found('Kim', ['', 'g:1']),
        found('Sam', ['', 'g:2']),
      ]).region,
      '',
    );
    expect(
      lights([
        found('Kim', ['', 'g:1', 'g:4']),
      ]).region,
      'g:4',
    );
    expect(
      lights([
        found('Kim', ['', 'g:1']),
        found('Ann', ['sky:32/5/6']),
      ]).region,
      '',
      reason: 'one in the sky: the top holds them all',
    );
  });

  test('lights the groups and sky cells holding results', () {
    // At full precision (8 << 28) + 3 is in cell 8 at level 4.
    final l = lights([
      found('Kim', ['', 'g:1', 'g:4']),
      found('Ann', ['sky:32/${(8 << 28) + 3}/${9 << 28}']),
    ]);
    expect(l.holdsGroup(group(1)), isTrue);
    expect(l.holdsGroup(group(4)), isTrue);
    expect(l.holdsGroup(group(2)), isFalse);
    expect(l.holdsCell(cell(4, 8, 9)), isTrue);
    expect(l.holdsCell(cell(4, 9, 9)), isFalse);
    expect(l.holdsCell(cell(32, (8 << 28) + 3, 9 << 28)), isTrue);
    expect(l.holdsCell(cell(5, 16, 18)), isTrue);
    expect(l.isEmpty, isFalse);
    expect(lights(const []).isEmpty, isTrue);
  });
}
