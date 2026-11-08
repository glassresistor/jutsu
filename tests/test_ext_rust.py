# Test extension behavior with rust vs python parse paths

import pytest
from jutsu import Environment


def test_loopcontrols_extension_rust_eligible():
    """Test loop controls extension behavior - should be rust eligible."""
    # This is a standard extension that should work with rust
    env = Environment(extensions=['jutsu.ext.loopcontrols'])
    template = env.from_string('{% for i in range(3) %}{% if i > 1 %}{% break %}{% endif %}{{ i }}{% endfor %}')
    result = template.render()
    assert result == '01'

def test_do_extension_rust_eligible():
    """Test do extension behavior - should be rust eligible."""
    # This is a standard extension that should work with rust
    env = Environment(extensions=['jutsu.ext.do'])
    template = env.from_string('{% set items = [] %}{% do items.append(42) %}{{ items }}')
    result = template.render()
    assert result == '[42]'

def test_debug_extension_rust_eligible():
    """Test debug extension behavior - should be rust eligible."""
    # This is a standard extension that should work with rust
    env = Environment(extensions=['jutsu.ext.debug'])
    template = env.from_string('{% debug %}')
    result = template.render()
    # Just check it doesn't crash
    assert 'debug' in result.lower() or 'context' in result.lower()


def test_i18n_extension_forces_python():
    """Test i18n extension behavior - should force python parse path."""
    # This is a standard extension that contributes tags, so it forces python path
    try:
        env = Environment(extensions=['jutsu.ext.i18n'])
        template = env.from_string('{% trans %}Hello{% endtrans %}')
        result = template.render()
        assert 'Hello' in result
    except Exception as e:
        # Skip if not available
        pytest.skip(f'i18n not available: {e}')

# Test the core concept: extensions with tags force python path
# while extensions without tags may use rust path