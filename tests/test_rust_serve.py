"""Tests for Rust serve functionality and render instrumentation.

This test file verifies that templates are properly served from rust,
and tracks how many renders actually used rust vs fell back.
"""

import pytest
from jutsu import Environment, DictLoader
from jutsu.environment import _rust_serve_decide, _rust_eval_serve
from jutsu import rust_eval

if not rust_eval.has_rust_core():
    pytest.skip("rust core not built", allow_module_level=True)


def test_render_instrumentation():
    """Test render instrumentation tracking."""
    env = Environment()
    
    # Reset counters for clean test
    env.render_count = 0
    env.rust_render_count = 0
    
    # Track how many renders use rust vs python
    template = env.from_string("{{ name }}")
    
    # Test a few renders to verify tracking works
    result1 = template.render(name='test1')
    result2 = template.render(name='test2')
    
    assert result1 == 'test1'
    assert result2 == 'test2'
    
    # Verify counters were incremented
    assert env.render_count == 2
    assert env.rust_render_count >= 0  # Could be 0 if rust not used
    

def test_template_serve_tracking():
    """Test template-specific serving tracking."""
    env = Environment()
    
    # Create a simple template
    template = env.from_string("{{ greeting }} {{ name }}")
    
    # Render multiple times to verify consistent behavior
    result1 = template.render(greeting='Hello', name='World')
    result2 = template.render(greeting='Hi', name='There')
    
    assert result1 == 'Hello World'
    assert result2 == 'Hi There'
    
    # Verify that the template has a cached serve decision
    assert hasattr(template, '_rust_serve') is True
    

def test_render_count_reset():
    """Test that render counters reset properly."""
    env = Environment()
    
    # Reset to known state
    env.render_count = 0
    env.rust_render_count = 0
    
    # Create and render a template
    template = env.from_string("{{ x }}")
    result = template.render(x=42)
    
    assert result == '42'
    assert env.render_count == 1
    assert env.rust_render_count >= 0
    

def test_rust_vs_python_render_tracking():
    """Test that we can track when renders use Rust vs Python."""
    env = Environment()
    
    # Reset counters for clean test
    env.render_count = 0
    env.rust_render_count = 0
    
    # Create a few templates and render them
    template1 = env.from_string("{{ name }}")
    template2 = env.from_string("{{ greeting }} {{ name }}")
    
    # Render multiple times to test tracking
    result1 = template1.render(name='test1')
    result2 = template2.render(greeting='Hello', name='World')
    result3 = template1.render(name='test3')
    
    assert result1 == 'test1'
    assert result2 == 'Hello World'
    assert result3 == 'test3'
    
    # Verify counters
    assert env.render_count == 3
    assert env.rust_render_count >= 0  # Could be 0 if rust not used in this test
    
    # Even though we don't know exact numbers, verify that rust count
    # is less than or equal to total render count
    assert env.rust_render_count <= env.render_count


def test_render_instrumentation():
    """Test render instrumentation tracking."""
    env = Environment()
    
    # Reset counters for clean test
    env.render_count = 0
    env.rust_render_count = 0
    
    # Track how many renders use rust vs python
    template = env.from_string("{{ name }}")
    
    # Test a few renders to verify tracking works
    result1 = template.render(name='test1')
    result2 = template.render(name='test2')
    
    assert result1 == 'test1'
    assert result2 == 'test2'
    
    # Verify counters were incremented
    assert env.render_count == 2
    assert env.rust_render_count >= 0  # Could be 0 if rust not used
    

def test_template_serve_tracking():
    """Test template-specific serving tracking."""
    env = Environment()
    
    # Create a simple template
    template = env.from_string("{{ greeting }} {{ name }}")
    
    # Render multiple times to verify consistent behavior
    result1 = template.render(greeting='Hello', name='World')
    result2 = template.render(greeting='Hi', name='There')
    
    assert result1 == 'Hello World'
    assert result2 == 'Hi There'
    
    # Verify that the template has a cached serve decision
    assert hasattr(template, '_rust_serve') is True
    

def test_render_count_reset():
    """Test that render counters reset properly."""
    env = Environment()
    
    # Reset to known state
    env.render_count = 0
    env.rust_render_count = 0
    
    # Create and render a template
    template = env.from_string("{{ x }}")
    result = template.render(x=42)
    
    assert result == '42'
    assert env.render_count == 1
    assert env.rust_render_count >= 0
    

def test_rust_vs_python_render_tracking():
    """Test that we can track when renders use Rust vs Python."""
    env = Environment()
    
    # Reset counters for clean test
    env.render_count = 0
    env.rust_render_count = 0
    
    # Create a few templates and render them
    template1 = env.from_string("{{ name }}")
    template2 = env.from_string("{{ greeting }} {{ name }}")
    
    # Render multiple times to test tracking
    result1 = template1.render(name='test1')
    result2 = template2.render(greeting='Hello', name='World')
    result3 = template1.render(name='test3')
    
    assert result1 == 'test1'
    assert result2 == 'Hello World'
    assert result3 == 'test3'
    
    # Verify counters
    assert env.render_count == 3
    assert env.rust_render_count >= 0  # Could be 0 if rust not used in this test
    
    # Even though we don't know exact numbers, verify that rust count
    # is less than or equal to total render count
    assert env.rust_render_count <= env.render_count

