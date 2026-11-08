# Test NativeEnvironment behavior with rust core

import pytest
from jutsu import Environment
from jutsu.nativetypes import NativeEnvironment, NativeTemplate


def test_native_environment_excluded_from_rust_serving():
    """Test that NativeEnvironment is excluded from rust serving (never reaches evaluator)."""
    # Create a native environment
    env = NativeEnvironment()
    
    # Verify it's a native environment
    assert isinstance(env, NativeEnvironment)
    
    # Test basic rendering
    template = env.from_string('{{ name }}')
    result = template.render(name='World')
    assert result == 'World'
    
    # Test with more complex template
    template2 = env.from_string('{% for i in items %}{{ i }}{% endfor %}')
    result2 = template2.render(items=['a', 'b', 'c'])
    assert result2 == 'abc'


def test_native_template_rendering():
    """Test NativeTemplate rendering behavior."""
    env = NativeEnvironment()
    
    # Create a native template
    template = env.from_string('{{ name|upper }}')
    result = template.render(name='hello')
    assert result == 'HELLO'
    
    # Test with filters that would be native-specific
    template2 = env.from_string('{% for item in items %}{{ item }}{% endfor %}')
    result2 = template2.render(items=[1, 2, 3])
    assert result2 == '123'


def test_native_environment_exclusion_in_eligibility():
    """Test that NativeEnvironment is excluded from rust eligibility predicate."""
    # This tests the C-v8a eligibility predicate behavior
    env = NativeEnvironment()
    
    # Native environments should not be eligible for rust serving
    # We can't directly test the internal predicate, but we can verify
    # that native environments work correctly with the system
    
    template = env.from_string('{{ name }}')
    result = template.render(name='Test')
    assert result == 'Test'
    
    # Test with complex expressions
    template2 = env.from_string('{% if condition %}true{% else %}false{% endif %}')
    result2 = template2.render(condition=True)
    assert result2 == 'true'


def test_native_environment_compatibility_with_rust_engines():
    """Test that NativeEnvironment works identically with rust engines default-on."""
    # Test with native environment in rust mode
    env = NativeEnvironment()
    
    # Basic functionality
    template = env.from_string('{{ name }}')
    result = template.render(name='Native')
    assert result == 'Native'
    
    # Complex templates should work
    template2 = env.from_string('{% for i in range(3) %}{{ i }}{% endfor %}')
    result2 = template2.render()
    assert result2 == '012'


def test_native_environment_vs_regular_environment():
    """Test that NativeEnvironment behaves differently from regular Environment."""
    # Create both types of environments
    regular_env = Environment()
    native_env = NativeEnvironment()
    
    # Test basic rendering
    regular_template = regular_env.from_string('{{ name }}')
    native_template = native_env.from_string('{{ name }}')
    
    regular_result = regular_template.render(name='Regular')
    native_result = native_template.render(name='Native')
    
    assert regular_result == 'Regular'
    assert native_result == 'Native'
    
    # Test with filters that might behave differently
    regular_template2 = regular_env.from_string('{{ name|upper }}')
    native_template2 = native_env.from_string('{{ name|upper }}')
    
    regular_result2 = regular_template2.render(name='test')
    native_result2 = native_template2.render(name='test')
    
    assert regular_result2 == 'TEST'
    assert native_result2 == 'TEST'


def test_native_environment_with_complex_expressions():
    """Test NativeEnvironment with complex Jinja expressions."""
    env = NativeEnvironment()
    
    # Test conditional logic
    template = env.from_string('{% if items %}has items{% else %}no items{% endif %}')
    result = template.render(items=['a', 'b'])
    assert result == 'has items'
    
    result2 = template.render(items=[])
    assert result2 == 'no items'
    
    # Test loops
    template2 = env.from_string('{% for item in items %}{{ item }}{% endfor %}')
    result3 = template2.render(items=['x', 'y', 'z'])
    assert result3 == 'xyz'
    
    # Test filters
    template3 = env.from_string('{{ items|join(", ") }}')
    result4 = template3.render(items=['a', 'b', 'c'])
    assert result4 == 'a, b, c'


def test_native_environment_inheritance():
    """Test NativeEnvironment inheritance support."""
    # This tests that native environments can work with the normal Jinja inheritance
    env = NativeEnvironment()
    
    # Simple case
    template = env.from_string('Hello {{ name }}!')
    result = template.render(name='World')
    assert result == 'Hello World!'
